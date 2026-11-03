use std::collections::HashMap;

use crate::branches::Entry;

pub struct Node {
    pub lane: usize,
    pub merge: bool,
}

pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub lane: usize,
}

#[derive(Default)]
pub struct Layout {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub lanes: usize,
}

impl Layout {
    pub fn new(entries: &[Entry]) -> Self {
        let indices: HashMap<_, _> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.commit.id.as_str(), index))
            .collect();
        let mut lanes: Vec<Option<&str>> = Vec::new();
        let mut layout = Self::default();
        for (index, entry) in entries.iter().enumerate() {
            let lane = if let Some(lane) = lanes
                .iter()
                .position(|id| *id == Some(entry.commit.id.as_str()))
            {
                lane
            } else {
                let lane = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                if lane == lanes.len() {
                    lanes.push(None);
                }
                lane
            };
            layout.nodes.push(Node {
                lane,
                merge: entry.parents.len() > 1,
            });
            lanes[lane] = None;
            for (parent_index, parent) in entry.parents.iter().enumerate() {
                let target_lane = if let Some(existing) =
                    lanes.iter().position(|id| *id == Some(parent.as_str()))
                {
                    existing
                } else {
                    let free = if parent_index == 0 && lanes[lane].is_none() {
                        lane
                    } else {
                        lanes
                            .iter()
                            .position(Option::is_none)
                            .unwrap_or(lanes.len())
                    };
                    if free == lanes.len() {
                        lanes.push(None);
                    }
                    lanes[free] = Some(parent);
                    free
                };
                layout.lanes = layout.lanes.max(target_lane + 1).max(lane + 1);
                layout.edges.push(Edge {
                    from: index,
                    to: indices
                        .get(parent.as_str())
                        .copied()
                        .unwrap_or(entries.len()),
                    lane: target_lane,
                });
            }
            layout.lanes = layout.lanes.max(lane + 1);
        }
        layout
    }
}
