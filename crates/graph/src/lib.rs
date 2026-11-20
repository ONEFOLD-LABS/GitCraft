//! Layout of the commit graph (issue #29, docs/ux/layout.md).
//!
//! Commits arrive newest first in topological order (every commit before its parents), page by
//! page. [`Graph::push`] places each one on a lane and returns the [`Row`] to draw: the node's
//! lane and the line segments in the upper half (from the row's top edge to its middle) and the
//! lower half (from the middle to the bottom edge). Lanes are columns; each line of history
//! keeps its lane and colour while it continues, so the picture stays stable while more pages
//! load. The layout of one commit costs O(lanes).
//!
//! Lanes are reused from the left once free, and trailing free lanes are dropped, so the graph
//! stays as narrow as the history allows.

/// A line segment inside one row, between lane `from` (at its start edge) and lane `to` (at its
/// end edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub from: usize,
    pub to: usize,
    /// Index into the theme's lane colours (wrap around with `%`).
    pub color: usize,
}

/// What to draw for one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The lane of the commit's node.
    pub lane: usize,
    /// Colour of the node.
    pub color: usize,
    /// Top edge → middle: lines passing by, and lines from children arriving at the node.
    pub top: Vec<Segment>,
    /// Middle → bottom edge: lines passing by, and lines from the node to its parents.
    pub bottom: Vec<Segment>,
    /// Number of lanes this row needs (for the column width).
    pub width: usize,
}

#[derive(Debug, Clone)]
struct Lane<T> {
    /// The commit this line of history leads to next.
    expect: T,
    color: usize,
}

/// Incremental layout state; push commits in display order.
#[derive(Debug, Clone)]
pub struct Graph<T> {
    lanes: Vec<Option<Lane<T>>>,
    next_color: usize,
}

impl<T: Clone + PartialEq> Default for Graph<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + PartialEq> Graph<T> {
    pub fn new() -> Self {
        Graph {
            lanes: Vec::new(),
            next_color: 0,
        }
    }

    fn new_color(&mut self) -> usize {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    fn free_slot(&mut self) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    fn lane_expecting(&self, id: &T) -> Option<usize> {
        self.lanes
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| &l.expect == id))
    }

    /// Places the next commit (`id` with its `parents`, first parent first) and returns its row.
    pub fn push(&mut self, id: &T, parents: &[T]) -> Row {
        let incoming: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| &l.expect == id))
            .map(|(i, _)| i)
            .collect();
        let width_top = self.lanes.len();

        // The node sits on the leftmost line that leads to it, or starts a new line (a tip).
        let (lane, color) = match incoming.first() {
            Some(&i) => (i, self.lanes[i].as_ref().map_or(0, |l| l.color)),
            None => {
                let slot = self.free_slot();
                (slot, self.new_color())
            }
        };

        let mut top = Vec::with_capacity(width_top);
        for (i, l) in self.lanes.iter().enumerate() {
            if let Some(l) = l {
                let to = if incoming.contains(&i) { lane } else { i };
                top.push(Segment {
                    from: i,
                    to,
                    color: l.color,
                });
            }
        }
        for &i in &incoming {
            self.lanes[i] = None;
        }

        // Lines passing by keep going.
        let mut bottom: Vec<Segment> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                l.as_ref().map(|l| Segment {
                    from: i,
                    to: i,
                    color: l.color,
                })
            })
            .collect();

        let mut seen: Vec<&T> = Vec::with_capacity(parents.len());
        for (n, p) in parents.iter().enumerate() {
            if seen.contains(&p) {
                continue;
            }
            seen.push(p);
            if let (0, Some(k)) = (n, self.lane_expecting(p))
                && k > lane
            {
                // The parent's line runs further right: it moves over into this (now free) lane
                // and keeps its colour, so the graph stays left-aligned.
                self.lanes[lane] = self.lanes[k].take();
                if let Some(s) = bottom.iter_mut().find(|s| s.from == k && s.to == k) {
                    s.to = lane;
                }
                bottom.push(Segment {
                    from: lane,
                    to: lane,
                    color,
                });
            } else if let Some(k) = self.lane_expecting(p) {
                // The parent's line already exists: join it.
                let c = if n == 0 {
                    color
                } else {
                    self.lanes[k].as_ref().map_or(color, |l| l.color)
                };
                bottom.push(Segment {
                    from: lane,
                    to: k,
                    color: c,
                });
            } else if n == 0 {
                // The first parent continues this line in the node's lane and colour.
                self.lanes[lane] = Some(Lane {
                    expect: p.clone(),
                    color,
                });
                bottom.push(Segment {
                    from: lane,
                    to: lane,
                    color,
                });
            } else {
                // Another parent of a merge starts its own line.
                let slot = self.free_slot();
                let c = self.new_color();
                self.lanes[slot] = Some(Lane {
                    expect: p.clone(),
                    color: c,
                });
                bottom.push(Segment {
                    from: lane,
                    to: slot,
                    color: c,
                });
            }
        }

        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        let width = width_top.max(self.lanes.len()).max(lane + 1);
        bottom.sort_by_key(|s| (s.to, s.from));
        Row {
            lane,
            color,
            top,
            bottom,
            width,
        }
    }

    /// Lanes in use after the last pushed commit.
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays out commits given as (id, parents), newest first.
    fn layout(commits: &[(u32, &[u32])]) -> Vec<Row> {
        let mut g = Graph::new();
        commits.iter().map(|(id, ps)| g.push(id, ps)).collect()
    }

    /// The lanes leaving a row at the bottom are exactly the lanes entering the next row at the
    /// top: no line starts or ends in the middle of nowhere.
    fn assert_continuous(rows: &[Row]) {
        for (n, pair) in rows.windows(2).enumerate() {
            let mut out: Vec<usize> = pair[0].bottom.iter().map(|s| s.to).collect();
            out.sort_unstable();
            out.dedup();
            let mut inn: Vec<usize> = pair[1].top.iter().map(|s| s.from).collect();
            inn.sort_unstable();
            inn.dedup();
            assert_eq!(out, inn, "rows {n}/{}", n + 1);
        }
        for r in rows {
            assert!(r.lane < r.width);
            for s in r.top.iter().chain(&r.bottom) {
                assert!(s.from < r.width && s.to < r.width, "{r:?}");
            }
        }
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let rows = layout(&[(3, &[2]), (2, &[1]), (1, &[])]);
        assert!(rows.iter().all(|r| r.lane == 0 && r.width == 1));
        assert!(rows.iter().all(|r| r.color == rows[0].color));
        assert!(rows[2].bottom.is_empty(), "root ends its line");
        assert_continuous(&rows);
    }

    #[test]
    fn branch_and_merge() {
        // 5 merges 4 (main) and 3 (side); both come from 2.
        let rows = layout(&[(5, &[4, 3]), (4, &[2]), (3, &[2]), (2, &[1]), (1, &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].bottom.len(), 2, "two lines leave the merge");
        assert_eq!(rows[1].lane, 0, "first parent stays in the lane");
        assert_eq!(rows[2].lane, 1, "second parent on its own lane");
        assert_ne!(rows[2].color, rows[1].color);
        assert!(
            rows[2].bottom.contains(&Segment {
                from: 1,
                to: 0,
                color: rows[2].color
            }),
            "the side line joins main right below its last commit"
        );
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[4].width, 1, "the side lane is gone again");
        assert_continuous(&rows);
    }

    #[test]
    fn several_tips_and_an_octopus_merge() {
        let rows = layout(&[
            (9, &[8]),
            (7, &[6]),
            (8, &[5, 6, 4, 3]),
            (6, &[2]),
            (5, &[2]),
            (4, &[2]),
            (3, &[2]),
            (2, &[1]),
            (1, &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1, "second tip gets the next lane");
        assert_continuous(&rows);
        assert_eq!(rows.last().map(|r| r.width), Some(1));
    }

    #[test]
    fn a_line_moves_left_into_a_freed_lane() {
        // Tip 3 (lane 0) and tip 2 (lane 1) share parent 1: 2's line ends, 1 moves to lane 0.
        let rows = layout(&[(4, &[3]), (5, &[2]), (3, &[1]), (2, &[1]), (1, &[])]);
        assert_eq!(rows.last().map(|r| (r.lane, r.width)), Some((0, 1)));
        assert_continuous(&rows);
    }

    #[test]
    fn criss_cross_merges() {
        let rows = layout(&[
            (6, &[4, 5]),
            (5, &[3, 2]),
            (4, &[2, 3]),
            (3, &[1]),
            (2, &[1]),
            (1, &[]),
        ]);
        assert_continuous(&rows);
    }

    #[test]
    fn duplicate_parents_and_missing_parents() {
        // A parent listed twice draws one line; a parent never pushed (shallow history, next
        // page) simply keeps its lane open.
        let rows = layout(&[(3, &[2, 2]), (2, &[99])]);
        assert_eq!(rows[0].bottom.len(), 1);
        assert_continuous(&rows);
    }

    /// Random histories: continuity and narrow graphs hold for every one.
    #[test]
    fn random_histories_stay_continuous() {
        let mut seed: u64 = 0x5eed;
        let mut next = move |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n.max(1)
        };
        for round in 0..200 {
            let n = 2 + next(60) as u32;
            // Commit k has parents among 0..k (older); newest first is n-1 .. 0.
            let mut commits: Vec<(u32, Vec<u32>)> = Vec::new();
            for k in 0..n {
                let mut ps = Vec::new();
                if k > 0 {
                    ps.push(k - 1 - next(u64::from(k).min(4)) as u32);
                    if next(4) == 0 {
                        ps.push(next(u64::from(k)) as u32);
                    }
                }
                commits.push((k, ps));
            }
            commits.reverse();
            let mut g = Graph::new();
            let rows: Vec<Row> = commits.iter().map(|(id, ps)| g.push(id, ps)).collect();
            assert_continuous(&rows);
            assert!(rows.iter().all(|r| r.width <= n as usize), "round {round}");
        }
    }

    /// `cargo test -p gitcraft-graph --release -- --ignored --nocapture`
    #[test]
    #[ignore = "measurement"]
    fn layout_of_100k_commits() {
        let n = 100_000u32;
        let commits: Vec<(u32, Vec<u32>)> = (0..n)
            .rev()
            .map(|k| {
                let mut ps = Vec::new();
                if k > 0 {
                    ps.push(k - 1);
                }
                if k > 10 && k % 7 == 0 {
                    ps.push(k - 9);
                }
                (k, ps)
            })
            .collect();
        let start = std::time::Instant::now();
        let mut g = Graph::new();
        let mut lanes = 0;
        for (id, ps) in &commits {
            lanes = lanes.max(g.push(id, ps).width);
        }
        println!("100k commits: {:?}, max {lanes} lanes", start.elapsed());
    }
}
