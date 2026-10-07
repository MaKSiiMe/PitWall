-- Custom rules of team-league.toml (sandboxed Lua: string, table, math).

-- Seed teams by name to make the bracket reproducible.
function seed(phase, ids)
  table.sort(ids)
  return ids
end

-- League points after each phase: 10 for the winner, then 6, 4, 3...
local scale = { 10, 6, 4, 3, 2, 1 }
function on_phase_end(phase, standings, points)
  for _, s in ipairs(standings) do
    points[s.id] = (points[s.id] or 0) + (scale[s.rank] or 0)
  end
  return { cup_points = points, messages = { "League points updated." } }
end
