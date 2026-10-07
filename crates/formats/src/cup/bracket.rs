//! Single elimination bracket.

use serde::{Deserialize, Serialize};

use crate::cup::standings::Standing;

/// One match of the bracket. `None` slots are byes or not yet decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketMatch {
    pub a: Option<String>,
    pub b: Option<String>,
    pub winner: Option<String>,
}

/// Rounds from the first to the final.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bracket {
    pub rounds: Vec<Vec<BracketMatch>>,
}

/// Standard seeding order for `size` slots (1 vs size, 2 vs size-1, ...), recursively balanced.
fn seed_order(size: usize) -> Vec<usize> {
    let mut order = vec![1usize];
    while order.len() < size {
        let n = order.len() * 2;
        order = order.iter().flat_map(|&s| [s, n + 1 - s]).collect();
    }
    order
}

impl Bracket {
    /// Builds a bracket from seeded ids (best first). Byes go to the best seeds.
    pub fn new(seeds: &[String]) -> Bracket {
        let size = seeds.len().next_power_of_two().max(2);
        let order = seed_order(size);
        let slot = |seed: usize| seeds.get(seed - 1).cloned();
        let first: Vec<BracketMatch> = order
            .chunks(2)
            .map(|pair| BracketMatch {
                a: slot(pair[0]),
                b: slot(pair[1]),
                winner: None,
            })
            .collect();
        let mut rounds = vec![first];
        while rounds.last().is_some_and(|r| r.len() > 1) {
            let len = rounds.last().map_or(0, Vec::len) / 2;
            rounds.push(vec![
                BracketMatch {
                    a: None,
                    b: None,
                    winner: None
                };
                len
            ]);
        }
        let mut bracket = Bracket { rounds };
        bracket.resolve_byes();
        bracket
    }

    /// Advances single-participant matches of the first round.
    fn resolve_byes(&mut self) {
        let byes: Vec<(usize, String)> = self.rounds[0]
            .iter()
            .enumerate()
            .filter_map(|(i, m)| match (&m.a, &m.b) {
                (Some(x), None) | (None, Some(x)) => Some((i, x.clone())),
                _ => None,
            })
            .collect();
        for (i, winner) in byes {
            self.set_winner(0, i, winner);
        }
    }

    fn set_winner(&mut self, round: usize, index: usize, winner: String) {
        self.rounds[round][index].winner = Some(winner.clone());
        if let Some(next) = self.rounds.get_mut(round + 1) {
            let m = &mut next[index / 2];
            if index % 2 == 0 {
                m.a = Some(winner);
            } else {
                m.b = Some(winner);
            }
        }
    }

    /// The next match to play: `(round, index)`.
    pub fn next_match(&self) -> Option<(usize, usize)> {
        self.rounds.iter().enumerate().find_map(|(r, matches)| {
            matches
                .iter()
                .position(|m| m.a.is_some() && m.b.is_some() && m.winner.is_none())
                .map(|i| (r, i))
        })
    }

    pub fn get(&self, round: usize, index: usize) -> Option<&BracketMatch> {
        self.rounds.get(round)?.get(index)
    }

    /// Records the winner of a match. Returns `false` if the winner is not in that match.
    pub fn record(&mut self, round: usize, index: usize, winner: &str) -> bool {
        let Some(m) = self.get(round, index) else {
            return false;
        };
        if m.a.as_deref() != Some(winner) && m.b.as_deref() != Some(winner) {
            return false;
        }
        self.set_winner(round, index, winner.to_owned());
        true
    }

    pub fn champion(&self) -> Option<&str> {
        self.rounds.last()?.first()?.winner.as_deref()
    }

    /// Final ranking: champion 1st, finalist 2nd, then by elimination round (ties share a rank).
    pub fn standings(&self, name_of: impl Fn(&str) -> String) -> Vec<Standing> {
        let mut out = Vec::new();
        if let Some(champion) = self.champion() {
            out.push((champion.to_owned(), 1u32));
        }
        let mut rank = 2u32;
        for round in self.rounds.iter().rev() {
            let losers: Vec<String> = round
                .iter()
                .filter_map(|m| {
                    let winner = m.winner.as_deref()?;
                    [&m.a, &m.b]
                        .into_iter()
                        .flatten()
                        .find(|x| x.as_str() != winner)
                        .cloned()
                })
                .collect();
            let count = u32::try_from(losers.len()).unwrap_or(0);
            for loser in losers {
                out.push((loser, rank));
            }
            rank += count;
        }
        out.into_iter()
            .map(|(id, rank)| Standing {
                name: name_of(&id),
                id,
                rank,
                points: 0,
                best_time: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("t{i}")).collect()
    }

    #[test]
    fn four_teams() {
        let mut b = Bracket::new(&ids(4));
        assert_eq!(b.rounds.len(), 2);
        // 1 vs 4, 2 vs 3
        assert_eq!(b.rounds[0][0].a.as_deref(), Some("t1"));
        assert_eq!(b.rounds[0][0].b.as_deref(), Some("t4"));
        assert_eq!(b.next_match(), Some((0, 0)));
        assert!(b.record(0, 0, "t1"));
        assert!(!b.record(0, 1, "t1"), "t1 is not in that match");
        assert!(b.record(0, 1, "t3"));
        assert_eq!(b.next_match(), Some((1, 0)));
        assert!(b.record(1, 0, "t3"));
        assert_eq!(b.champion(), Some("t3"));
        assert_eq!(b.next_match(), None);
        let s = b.standings(str::to_owned);
        let ranks: Vec<_> = s.iter().map(|s| (s.id.as_str(), s.rank)).collect();
        assert_eq!(ranks, [("t3", 1), ("t1", 2), ("t4", 3), ("t2", 3)]);
    }

    #[test]
    fn byes_for_top_seeds() {
        let b = Bracket::new(&ids(3));
        // t1 gets a bye and is already in the final slot.
        assert_eq!(b.rounds[0][0].winner.as_deref(), Some("t1"));
        assert_eq!(b.rounds[1][0].a.as_deref(), Some("t1"));
        assert_eq!(b.next_match(), Some((0, 1)));
    }

    #[test]
    fn two_teams_is_a_single_final() {
        let mut b = Bracket::new(&ids(2));
        assert_eq!(b.rounds.len(), 1);
        assert!(b.record(0, 0, "t2"));
        assert_eq!(b.champion(), Some("t2"));
    }
}
