//! Commit-graph lane layout for the History view — a port of the desktop
//! `history-graph.ts` algorithm, kept as pure data for the native renderer.
//! Lane order is Git topology: a lane holds the commit id expected next plus
//! the colour of its line; parents fan out into free lanes, each taking the
//! next colour in rotation.

use app_core::git_view::GitViewCommit;

/// One branch-line colour as sRGB bytes. The desktop palette is authored in
/// HSL (`hsl(0, 75%, 55%)` … `hsl(335, 75%, 58%)`) at mid luminance so the
/// lines read on light and dark backgrounds; these are the same colours
/// converted for the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl GraphColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> GraphColor {
        GraphColor { r, g, b }
    }
}

/// Branch-line palette; rows and edges carry indices into this table.
pub const GRAPH_PALETTE: [GraphColor; 8] = [
    GraphColor::rgb(226, 54, 54),  // hsl(0, 75%, 55%)
    GraphColor::rgb(244, 140, 37), // hsl(30, 90%, 55%)
    GraphColor::rgb(239, 200, 6),  // hsl(50, 95%, 48%)
    GraphColor::rgb(46, 158, 93),  // hsl(145, 55%, 40%)
    GraphColor::rgb(21, 164, 193), // hsl(190, 80%, 42%)
    GraphColor::rgb(57, 118, 239), // hsl(220, 85%, 58%)
    GraphColor::rgb(160, 78, 218), // hsl(275, 65%, 58%)
    GraphColor::rgb(228, 68, 135), // hsl(335, 75%, 58%)
];

/// A curved edge from a commit node down into a parent's lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    /// Lane the edge leaves from — the commit's own lane.
    pub from_lane: usize,
    /// Lane the parent's line occupies below this row.
    pub to_lane: usize,
    /// Palette index colouring the edge and the line it joins.
    pub color: usize,
}

/// Another lane whose line passes vertically through this row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphThrough {
    pub lane: usize,
    /// Palette index of the passing line.
    pub color: usize,
}

/// One commit's graph geometry; pure data for the renderer, no drawing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphRow {
    /// Lane index of the commit's node.
    pub lane: usize,
    /// Palette index of the commit's branch line.
    pub color: usize,
    /// Whether the commit is the checkout's HEAD.
    pub head: bool,
    /// Whether the commit has two or more parents.
    pub merge: bool,
    /// Whether a line already waits for this commit, so one enters its node
    /// from above.
    pub incoming: bool,
    /// Other lanes whose lines pass vertically through this row.
    pub through: Vec<GraphThrough>,
    /// One edge per parent, in parent order.
    pub edges: Vec<GraphEdge>,
}

/// The laid-out graph: one row per commit plus the lane count to size the
/// graph column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    pub lanes: usize,
}

/// Lays out `commits` (already in Git topological order) into lanes, colours
/// and edges — the same result as the desktop `layoutGraph`.
pub fn layout_graph(commits: &[GitViewCommit]) -> GraphLayout {
    // Lane → id of the commit expected next there; `None` is a free lane.
    let mut expected: Vec<Option<&str>> = Vec::new();
    // Lane → palette index of its line; always the same length as `expected`.
    let mut colors: Vec<usize> = Vec::new();
    let mut next_color = 0usize;
    let mut lane_count = 0usize;
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let before = expected.clone();
        let found = expected
            .iter()
            .position(|slot| *slot == Some(commit.id.as_str()));
        let incoming = found.is_some();
        let (lane, color) = match found {
            Some(lane) => (lane, colors[lane]),
            None => {
                let color = next_color;
                next_color = (next_color + 1) % GRAPH_PALETTE.len();
                match expected.iter().position(|slot| slot.is_none()) {
                    Some(free) => {
                        expected[free] = Some(commit.id.as_str());
                        colors[free] = color;
                        (free, color)
                    }
                    None => {
                        expected.push(Some(commit.id.as_str()));
                        colors.push(color);
                        (expected.len() - 1, color)
                    }
                }
            }
        };

        // A first parent that another lane already waits for is joined by an
        // edge, not tracked twice; a duplicate lane would never be freed.
        let mut parents = commit.parents.iter().map(String::as_str);
        let first = parents.next();
        expected[lane] = match first {
            Some(first) if !expected.contains(&Some(first)) => Some(first),
            _ => None,
        };
        for parent in parents {
            if expected.contains(&Some(parent)) {
                continue;
            }
            let parent_color = next_color;
            next_color = (next_color + 1) % GRAPH_PALETTE.len();
            match expected.iter().position(|slot| slot.is_none()) {
                Some(free) => {
                    expected[free] = Some(parent);
                    colors[free] = parent_color;
                }
                None => {
                    expected.push(Some(parent));
                    colors.push(parent_color);
                }
            }
        }
        while expected.last().is_some_and(|slot| slot.is_none()) {
            expected.pop();
            colors.pop();
        }
        let after = expected.clone();
        lane_count = lane_count.max(before.len()).max(after.len()).max(lane + 1);

        let mut through = Vec::new();
        for (index, slot) in after.iter().enumerate() {
            if index == lane || before.get(index).copied().flatten().is_none() || slot.is_none() {
                continue;
            }
            through.push(GraphThrough {
                lane: index,
                color: colors[index],
            });
        }
        let mut edges = Vec::new();
        for parent in &commit.parents {
            if let Some(to_lane) = after.iter().position(|slot| *slot == Some(parent.as_str())) {
                edges.push(GraphEdge {
                    from_lane: lane,
                    to_lane,
                    color: colors[to_lane],
                });
            }
        }

        rows.push(GraphRow {
            lane,
            color,
            head: commit.head,
            merge: commit.parents.len() > 1,
            incoming,
            through,
            edges,
        });
    }

    GraphLayout {
        rows,
        lanes: lane_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str], head: bool) -> GitViewCommit {
        GitViewCommit {
            id: id.to_string(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
            refs: Vec::new(),
            head,
            author: "A U Thor".to_string(),
            time: 0,
            subject: format!("commit {id}"),
        }
    }

    fn commits(specs: &[(&str, &[&str], bool)]) -> Vec<GitViewCommit> {
        specs
            .iter()
            .map(|(id, parents, head)| commit(id, parents, *head))
            .collect()
    }

    #[test]
    fn palette_has_eight_entries() {
        assert_eq!(GRAPH_PALETTE.len(), 8);
    }

    #[test]
    fn keeps_one_colour_along_a_first_parent_line() {
        let layout = layout_graph(&commits(&[
            ("a", &["b"], false),
            ("b", &["c"], false),
            ("c", &["d"], false),
            ("d", &[], false),
        ]));
        assert_eq!(layout.lanes, 1);
        assert_eq!(
            layout.rows.iter().map(|row| row.lane).collect::<Vec<_>>(),
            vec![0, 0, 0, 0]
        );
        assert_eq!(
            layout.rows.iter().map(|row| row.color).collect::<Vec<_>>(),
            vec![0, 0, 0, 0]
        );
        assert_eq!(
            layout.rows[0].edges,
            vec![GraphEdge {
                from_lane: 0,
                to_lane: 0,
                color: 0
            }]
        );
        assert!(layout.rows[0].through.is_empty());
        assert!(layout.rows[3].edges.is_empty());
    }

    #[test]
    fn merge_second_parent_gets_a_new_colour() {
        let layout = layout_graph(&commits(&[
            ("m", &["p1", "p2"], false),
            ("p1", &["x"], false),
            ("p2", &["y"], false),
            ("x", &[], false),
            ("y", &[], false),
        ]));
        assert_eq!(layout.lanes, 2);
        assert!(layout.rows[0].merge);
        assert_eq!(
            layout.rows[0].edges,
            vec![
                GraphEdge {
                    from_lane: 0,
                    to_lane: 0,
                    color: 0
                },
                GraphEdge {
                    from_lane: 0,
                    to_lane: 1,
                    color: 1
                },
            ]
        );
        assert_eq!((layout.rows[1].lane, layout.rows[1].color), (0, 0));
        assert_eq!((layout.rows[2].lane, layout.rows[2].color), (1, 1));
        assert_eq!((layout.rows[4].lane, layout.rows[4].color), (1, 1));
    }

    #[test]
    fn reuses_a_freed_lane_but_takes_the_next_colour() {
        let layout = layout_graph(&commits(&[
            ("m", &["p1", "p2"], false),
            ("p1", &["x"], false),
            ("p2", &[], false),
            ("u", &[], false),
            ("x", &[], false),
        ]));
        assert_eq!((layout.rows[2].lane, layout.rows[2].color), (1, 1));
        assert_eq!((layout.rows[3].lane, layout.rows[3].color), (1, 2));
    }

    #[test]
    fn root_commit_ends_its_line() {
        let layout = layout_graph(&commits(&[("a", &["b"], false), ("b", &[], false)]));
        assert_eq!((layout.rows[1].lane, layout.rows[1].color), (0, 0));
        assert!(layout.rows[1].edges.is_empty());
        assert!(layout.rows[1].through.is_empty());
        assert_eq!(layout.lanes, 1);
    }

    #[test]
    fn flags_head_and_merge() {
        let layout = layout_graph(&commits(&[
            ("h", &["m"], true),
            ("m", &["r", "s"], false),
            ("r", &[], false),
            ("s", &[], false),
        ]));
        let flags: Vec<(bool, bool)> = layout
            .rows
            .iter()
            .map(|row| (row.head, row.merge))
            .collect();
        assert_eq!(
            flags,
            vec![(true, false), (false, true), (false, false), (false, false)]
        );
    }

    #[test]
    fn parent_outside_the_loaded_page_leaves_an_open_edge() {
        let layout = layout_graph(&commits(&[("a", &["b", "x"], false), ("b", &[], false)]));
        assert_eq!(layout.lanes, 2);
        assert_eq!(
            layout.rows[0].edges,
            vec![
                GraphEdge {
                    from_lane: 0,
                    to_lane: 0,
                    color: 0
                },
                GraphEdge {
                    from_lane: 0,
                    to_lane: 1,
                    color: 1
                },
            ]
        );
        assert_eq!(
            layout.rows[1].through,
            vec![GraphThrough { lane: 1, color: 1 }]
        );
    }

    #[test]
    fn curves_a_tip_into_the_lane_that_already_waits_for_its_parent() {
        // a and b are separate tips that both descend from p.
        let layout = layout_graph(&commits(&[
            ("a", &["p"], false),
            ("b", &["p"], false),
            ("p", &["q"], false),
            ("q", &[], false),
        ]));
        assert_eq!(layout.lanes, 2);
        assert_eq!(layout.rows[1].lane, 1);
        // b's first parent is already tracked in lane 0: its edge curves
        // there and lane 1 is not kept open.
        assert_eq!(
            layout.rows[1].edges,
            vec![GraphEdge {
                from_lane: 1,
                to_lane: 0,
                color: layout.rows[0].color,
            }]
        );
        assert_eq!(layout.rows[2].lane, 0);
        assert!(layout.rows[2].through.is_empty());
        assert!(layout.rows[3].through.is_empty());
    }

    #[test]
    fn does_not_keep_a_second_lane_open_when_merge_parents_share_a_base() {
        // a's merge brings p1 and p2; both lines continue to the shared base.
        let layout = layout_graph(&commits(&[
            ("a", &["p1", "p2"], false),
            ("p1", &["base"], false),
            ("p2", &["base"], false),
            ("base", &[], false),
        ]));
        assert_eq!(
            layout.rows[2].edges,
            vec![GraphEdge {
                from_lane: 1,
                to_lane: 0,
                color: layout.rows[0].edges[0].color,
            }]
        );
        assert_eq!(layout.rows[3].lane, 0);
        assert!(layout.rows[3].through.is_empty());
    }

    #[test]
    fn flags_whether_a_line_enters_the_commit_from_above() {
        let layout = layout_graph(&commits(&[("a", &["b"], false), ("b", &[], false)]));
        assert_eq!(
            layout
                .rows
                .iter()
                .map(|row| row.incoming)
                .collect::<Vec<_>>(),
            vec![false, true]
        );
    }

    #[test]
    fn octopus_parents_fan_out_in_order() {
        let layout = layout_graph(&commits(&[
            ("m", &["p1", "p2", "p3"], false),
            ("p1", &[], false),
            ("p2", &[], false),
            ("p3", &[], false),
        ]));
        assert!(layout.rows[0].merge);
        assert_eq!(layout.lanes, 3);
        assert_eq!(
            layout.rows[0].edges,
            vec![
                GraphEdge {
                    from_lane: 0,
                    to_lane: 0,
                    color: 0
                },
                GraphEdge {
                    from_lane: 0,
                    to_lane: 1,
                    color: 1
                },
                GraphEdge {
                    from_lane: 0,
                    to_lane: 2,
                    color: 2
                },
            ]
        );
        assert_eq!(
            layout
                .rows
                .iter()
                .map(|row| (row.lane, row.color))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 0), (1, 1), (2, 2)]
        );
    }

    #[test]
    fn empty_input_produces_no_rows() {
        let layout = layout_graph(&[]);
        assert!(layout.rows.is_empty());
        assert_eq!(layout.lanes, 0);
    }
}
