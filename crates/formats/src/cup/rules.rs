//! Custom cup rules.
//!
//! A cup may point to a Lua file defining any of these functions:
//!
//! ```lua
//! -- Called by `qualify = { lua = "top_half" }`: returns the ids going to the next phase.
//! function top_half(phase, standings)
//!   local out = {}
//!   for i = 1, math.ceil(#standings / 2) do out[#out + 1] = standings[i].id end
//!   return out
//! end
//!
//! -- Optional: reorders the participants of a phase (best seed first).
//! function seed(phase, ids) return ids end
//!
//! -- Optional: cumulative cup points and chat messages after each phase.
//! function on_phase_end(phase, standings, cup_points)
//!   for _, s in ipairs(standings) do
//!     cup_points[s.id] = (cup_points[s.id] or 0) + math.max(0, 10 - s.rank)
//!   end
//!   return { cup_points = cup_points, messages = { "Phase " .. phase .. " done" } }
//! end
//! ```
//!
//! `standings` is an array of `{ id, name, rank, points, best_time }`.
//! The Lua state is sandboxed: only `string`, `table` and `math`, no files, no network, a memory
//! limit and an instruction budget per call.

use std::collections::BTreeMap;

use crate::cup::standings::Standing;

/// Errors raised by custom rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    #[error("rules script error: {0}")]
    Script(String),
    #[error("rules function {0:?} is not defined")]
    MissingFunction(String),
    #[error("Lua support is not compiled in (enable the `lua` feature)")]
    Unsupported,
}

/// What `on_phase_end` may return.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseEndOutcome {
    pub cup_points: Option<BTreeMap<String, i64>>,
    pub messages: Vec<String>,
}

/// Hooks a cup can customize.
pub trait CupRules: Send + Sync + std::fmt::Debug {
    /// Runs a named qualification function.
    fn qualify(
        &self,
        function: &str,
        phase: &str,
        standings: &[Standing],
    ) -> Result<Vec<String>, RulesError>;
    /// Reorders participants; `None` when the script defines no `seed`.
    fn seed(&self, phase: &str, ids: &[String]) -> Result<Option<Vec<String>>, RulesError>;
    /// Called after each phase.
    fn on_phase_end(
        &self,
        phase: &str,
        standings: &[Standing],
        cup_points: &BTreeMap<String, i64>,
    ) -> Result<PhaseEndOutcome, RulesError>;
}

#[cfg(feature = "lua")]
pub use lua::LuaRules;

#[cfg(feature = "lua")]
mod lua {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};

    use super::{CupRules, PhaseEndOutcome, RulesError};
    use crate::cup::standings::Standing;

    /// Instructions allowed per call (counted by blocks of `HOOK_EVERY`).
    const BUDGET: i64 = 50_000_000;
    const HOOK_EVERY: u32 = 10_000;
    const MEMORY_LIMIT: usize = 32 * 1024 * 1024;

    /// Rules implemented in a sandboxed Lua script.
    pub struct LuaRules {
        lua: Lua,
        budget: Arc<AtomicI64>,
    }

    impl std::fmt::Debug for LuaRules {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("LuaRules").finish_non_exhaustive()
        }
    }

    fn script_err(err: mlua::Error) -> RulesError {
        RulesError::Script(err.to_string())
    }

    impl LuaRules {
        /// Loads and runs a rules script (its top level defines the functions).
        pub fn load(source: &str, name: &str) -> Result<LuaRules, RulesError> {
            let lua = Lua::new_with(
                StdLib::STRING | StdLib::TABLE | StdLib::MATH,
                LuaOptions::default(),
            )
            .map_err(script_err)?;
            lua.set_memory_limit(MEMORY_LIMIT).map_err(script_err)?;
            let budget = Arc::new(AtomicI64::new(BUDGET));
            let hook_budget = Arc::clone(&budget);
            lua.set_hook(
                HookTriggers::new().every_nth_instruction(HOOK_EVERY),
                move |_, _| {
                    if hook_budget.fetch_sub(i64::from(HOOK_EVERY), Ordering::Relaxed) <= 0 {
                        return Err(mlua::Error::RuntimeError(
                            "instruction budget exceeded (infinite loop?)".to_owned(),
                        ));
                    }
                    Ok(VmState::Continue)
                },
            )
            .map_err(script_err)?;

            let globals = lua.globals();
            for unsafe_fn in ["dofile", "loadfile", "load", "collectgarbage"] {
                globals.set(unsafe_fn, Value::Nil).map_err(script_err)?;
            }
            let log_name = name.to_owned();
            let print = lua
                .create_function(move |_, args: mlua::Variadic<String>| {
                    tracing::info!(target: "pitwall::cup_rules", script = %log_name, "{}", args.join(" "));
                    Ok(())
                })
                .map_err(script_err)?;
            globals.set("print", print).map_err(script_err)?;

            let rules = LuaRules { lua, budget };
            rules.reset_budget();
            rules
                .lua
                .load(source)
                .set_name(name)
                .exec()
                .map_err(script_err)?;
            Ok(rules)
        }

        fn reset_budget(&self) {
            self.budget.store(BUDGET, Ordering::Relaxed);
        }

        fn function(&self, name: &str) -> Result<Option<Function>, RulesError> {
            match self.lua.globals().get::<Value>(name).map_err(script_err)? {
                Value::Function(f) => Ok(Some(f)),
                Value::Nil => Ok(None),
                _ => Err(RulesError::Script(format!("{name} is not a function"))),
            }
        }

        fn standings(&self, standings: &[Standing]) -> Result<Value, RulesError> {
            let table = self.lua.create_table().map_err(script_err)?;
            for (i, s) in standings.iter().enumerate() {
                let row = self.lua.create_table().map_err(script_err)?;
                row.set("id", s.id.as_str()).map_err(script_err)?;
                row.set("name", s.name.as_str()).map_err(script_err)?;
                row.set("rank", s.rank).map_err(script_err)?;
                row.set("points", s.points).map_err(script_err)?;
                row.set("best_time", s.best_time).map_err(script_err)?;
                table.set(i + 1, row).map_err(script_err)?;
            }
            Ok(Value::Table(table))
        }
    }

    impl CupRules for LuaRules {
        fn qualify(
            &self,
            function: &str,
            phase: &str,
            standings: &[Standing],
        ) -> Result<Vec<String>, RulesError> {
            let f = self
                .function(function)?
                .ok_or_else(|| RulesError::MissingFunction(function.to_owned()))?;
            self.reset_budget();
            let result: Value = f
                .call((phase, self.standings(standings)?))
                .map_err(script_err)?;
            self.lua.from_value(result).map_err(script_err)
        }

        fn seed(&self, phase: &str, ids: &[String]) -> Result<Option<Vec<String>>, RulesError> {
            let Some(f) = self.function("seed")? else {
                return Ok(None);
            };
            self.reset_budget();
            let ids = self.lua.to_value(ids).map_err(script_err)?;
            let result: Value = f.call((phase, ids)).map_err(script_err)?;
            self.lua.from_value(result).map(Some).map_err(script_err)
        }

        fn on_phase_end(
            &self,
            phase: &str,
            standings: &[Standing],
            cup_points: &BTreeMap<String, i64>,
        ) -> Result<PhaseEndOutcome, RulesError> {
            let Some(f) = self.function("on_phase_end")? else {
                return Ok(PhaseEndOutcome::default());
            };
            self.reset_budget();
            let points = self.lua.create_table().map_err(script_err)?;
            for (id, p) in cup_points {
                points.set(id.as_str(), *p).map_err(script_err)?;
            }
            let result: Value = f
                .call((phase, self.standings(standings)?, points))
                .map_err(script_err)?;
            let Value::Table(table) = result else {
                return Ok(PhaseEndOutcome::default());
            };
            let cup_points: Option<BTreeMap<String, i64>> =
                match table.get::<Value>("cup_points").map_err(script_err)? {
                    Value::Nil => None,
                    v => Some(self.lua.from_value(v).map_err(script_err)?),
                };
            let messages: Vec<String> = match table.get::<Value>("messages").map_err(script_err)? {
                Value::Nil => Vec::new(),
                v => self.lua.from_value(v).map_err(script_err)?,
            };
            Ok(PhaseEndOutcome {
                cup_points,
                messages,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn standings() -> Vec<Standing> {
            ["a", "b", "c", "d", "e"]
                .iter()
                .enumerate()
                .map(|(i, id)| Standing {
                    id: (*id).to_owned(),
                    name: id.to_uppercase(),
                    rank: u32::try_from(i + 1).unwrap(),
                    points: 10 - i32::try_from(i).unwrap(),
                    best_time: Some(50_000 + i32::try_from(i).unwrap()),
                })
                .collect()
        }

        const SCRIPT: &str = r#"
function top_half(phase, standings)
  local out = {}
  for i = 1, math.ceil(#standings / 2) do out[#out + 1] = standings[i].id end
  return out
end

function seed(phase, ids)
  local out = {}
  for i = #ids, 1, -1 do out[#out + 1] = ids[i] end
  return out
end

function on_phase_end(phase, standings, cup_points)
  for _, s in ipairs(standings) do
    cup_points[s.id] = (cup_points[s.id] or 0) + math.max(0, 6 - s.rank)
  end
  return { cup_points = cup_points, messages = { "end of " .. phase } }
end
"#;

        #[test]
        fn runs_the_documented_hooks() {
            let rules = LuaRules::load(SCRIPT, "test.lua").unwrap();
            assert_eq!(
                rules.qualify("top_half", "quali", &standings()).unwrap(),
                ["a", "b", "c"]
            );
            assert_eq!(
                rules.seed("final", &["x".into(), "y".into()]).unwrap(),
                Some(vec!["y".to_owned(), "x".to_owned()])
            );
            let mut points = BTreeMap::new();
            points.insert("a".to_owned(), 3);
            let outcome = rules.on_phase_end("quali", &standings(), &points).unwrap();
            let cup_points = outcome.cup_points.unwrap();
            assert_eq!(cup_points["a"], 8);
            assert_eq!(cup_points["e"], 1);
            assert_eq!(outcome.messages, ["end of quali"]);
        }

        #[test]
        fn missing_functions() {
            let rules = LuaRules::load("x = 1", "empty.lua").unwrap();
            assert_eq!(rules.seed("p", &[]).unwrap(), None);
            assert_eq!(
                rules.on_phase_end("p", &[], &BTreeMap::new()).unwrap(),
                PhaseEndOutcome::default()
            );
            assert!(matches!(
                rules.qualify("nope", "p", &[]),
                Err(RulesError::MissingFunction(_))
            ));
        }

        #[test]
        fn sandbox_blocks_io_and_infinite_loops() {
            assert!(LuaRules::load("io.open('/etc/passwd')", "io.lua").is_err());
            assert!(LuaRules::load("os.execute('ls')", "os.lua").is_err());
            assert!(LuaRules::load("dofile('/etc/passwd')", "dofile.lua").is_err());
            assert!(LuaRules::load("require('socket')", "require.lua").is_err());
            let rules =
                LuaRules::load("function forever(p, s) while true do end end", "loop.lua").unwrap();
            let err = rules.qualify("forever", "p", &[]).unwrap_err();
            assert!(err.to_string().contains("budget"), "{err}");
            // The state is still usable after an aborted call.
            let rules2 = LuaRules::load(SCRIPT, "ok.lua").unwrap();
            assert!(rules2.qualify("top_half", "q", &standings()).is_ok());
        }

        #[test]
        fn syntax_errors_are_reported() {
            let err = LuaRules::load("function (", "bad.lua").unwrap_err();
            assert!(matches!(err, RulesError::Script(_)));
        }
    }
}
