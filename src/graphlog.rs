// Copyright 2020 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::hash::Hash;
use std::io::Write;
use std::{cmp, io};

use itertools::Itertools;
use jujutsu_lib::settings::UserSettings;
use renderdag::{Ancestor, GraphRowRenderer, Renderer};

#[derive(Debug, Clone, PartialEq, Eq)]
// An edge to another node in the graph
pub enum Edge<T> {
    Present { target: T, direct: bool },
    Missing,
}

impl<T> Edge<T> {
    pub fn missing() -> Self {
        Edge::Missing
    }

    pub fn direct(id: T) -> Self {
        Edge::Present {
            target: id,
            direct: true,
        }
    }

    pub fn indirect(id: T) -> Self {
        Edge::Present {
            target: id,
            direct: false,
        }
    }
}

pub trait GraphLog<K: Clone + Eq + Hash> {
    fn add_node(
        &mut self,
        id: &K,
        edges: &[Edge<K>],
        node_symbol: &str,
        text: &str,
    ) -> io::Result<()>;

    fn default_node_symbol(&self) -> &str;

    fn width(&self, id: &K, edges: &[Edge<K>]) -> usize;
}

pub struct SaplingGraphLog<'writer, R> {
    renderer: R,
    writer: &'writer mut dyn Write,
    default_node_symbol: String,
}

impl<K: Clone> From<&Edge<K>> for Ancestor<K> {
    fn from(e: &Edge<K>) -> Self {
        match e {
            Edge::Present {
                target,
                direct: true,
            } => Ancestor::Parent(target.clone()),
            Edge::Present { target, .. } => Ancestor::Ancestor(target.clone()),
            Edge::Missing => Ancestor::Anonymous,
        }
    }
}

impl<'writer, K, R> GraphLog<K> for SaplingGraphLog<'writer, R>
where
    K: Clone + Eq + Hash,
    R: Renderer<K, Output = String>,
{
    fn add_node(
        &mut self,
        id: &K,
        edges: &[Edge<K>],
        node_symbol: &str,
        text: &str,
    ) -> io::Result<()> {
        let row = self.renderer.next_row(
            id.clone(),
            edges.iter().map_into().collect(),
            node_symbol.into(),
            text.into(),
        );

        write!(self.writer, "{row}")
    }

    fn default_node_symbol(&self) -> &str {
        &self.default_node_symbol
    }

    fn width(&self, id: &K, edges: &[Edge<K>]) -> usize {
        let parents = edges.iter().map_into().collect();
        let w: u64 = self.renderer.width(Some(id), Some(&parents));
        w.try_into().unwrap()
    }
}

impl<'writer, R> SaplingGraphLog<'writer, R> {
    pub fn create<K>(
        renderer: R,
        formatter: &'writer mut dyn Write,
        default_node_symbol: &str,
    ) -> Box<dyn GraphLog<K> + 'writer>
    where
        K: Clone + Eq + Hash + 'writer,
        R: Renderer<K, Output = String> + 'writer,
    {
        Box::new(SaplingGraphLog {
            renderer,
            writer: formatter,
            default_node_symbol: default_node_symbol.to_owned(),
        })
    }
}

pub fn get_graphlog<'a, K: Clone + Eq + Hash + 'a>(
    settings: &UserSettings,
    formatter: &'a mut dyn Write,
) -> Box<dyn GraphLog<K> + 'a> {
    let builder = GraphRowRenderer::new().output().with_min_row_height(0);

    match settings.graph_style().as_str() {
        "curved" => SaplingGraphLog::create(builder.build_box_drawing(), formatter, "o"),
        "square" => SaplingGraphLog::create(
            builder.build_box_drawing().with_square_glyphs(),
            formatter,
            "o",
        ),
        "ascii" => SaplingGraphLog::create(builder.build_ascii(), formatter, "o"),
        "ascii-large" => SaplingGraphLog::create(builder.build_ascii_large(), formatter, "o"),
        _ => Box::new(AsciiGraphDrawer::new(formatter)),
    }
}

pub struct AsciiGraphDrawer<'writer, K> {
    writer: &'writer mut dyn Write,
    edges: Vec<Edge<K>>,
    pending_text: Vec<String>,
}

impl<'writer, K> AsciiGraphDrawer<'writer, K>
where
    K: Clone + Eq + Hash,
{
    pub fn new(writer: &'writer mut dyn Write) -> Self {
        Self {
            writer,
            edges: Default::default(),
            pending_text: Default::default(),
        }
    }
}

impl<'writer, K: Clone + Eq + Hash> GraphLog<K> for AsciiGraphDrawer<'writer, K> {
    fn add_node(
        &mut self,
        id: &K,
        edges: &[Edge<K>],
        node_symbol: &str,
        text: &str,
    ) -> io::Result<()> {
        assert!(self.pending_text.is_empty());
        for line in text.split(|x| x == '\n') {
            self.pending_text.push(line.into());
        }
        if self.pending_text.last() == Some(&"".into()) {
            self.pending_text.pop().unwrap();
        }
        self.pending_text.reverse();

        // Check if an existing edge should be terminated by the new node. If there
        // is, draw the new node in the same column. Otherwise, insert it at the right.
        let edge_index = if let Some(edge_index) = self.index_by_target(id) {
            // This edge terminates in the node we're adding

            // If we're inserting a merge somewhere that's not the very right, the edges
            // right of it will move further right, so we need to prepare by inserting rows
            // of '\'.
            if edges.len() > 2 && edge_index < self.edges.len() - 1 {
                for i in 2..edges.len() {
                    for edge in self.edges.iter().take(edge_index + 1) {
                        AsciiGraphDrawer::straight_edge(&mut self.writer, edge)?;
                    }
                    for _ in 0..i - 2 {
                        write!(self.writer, "  ")?;
                    }
                    for _ in edge_index + 1..self.edges.len() {
                        write!(self.writer, " \\")?;
                    }
                    writeln!(self.writer)?;
                }
            }

            self.edges.remove(edge_index);
            edge_index
        } else {
            self.edges.len()
        };

        // Draw the edges to the left of the new node
        for edge in self.edges.iter().take(edge_index) {
            AsciiGraphDrawer::straight_edge(&mut self.writer, edge)?;
        }
        // Draw the new node
        write!(self.writer, "{node_symbol}")?;
        // If it's a merge of many nodes, draw a vertical line to the right
        for _ in 3..edges.len() {
            write!(self.writer, "--")?;
        }
        if edges.len() > 2 {
            write!(self.writer, "-.")?;
        }
        write!(self.writer, " ")?;
        // Draw the edges to the right of the new node
        for edge in self.edges.iter().skip(edge_index) {
            AsciiGraphDrawer::straight_edge(&mut self.writer, edge)?;
        }
        if edges.len() > 1 {
            write!(self.writer, "  ")?;
        }

        self.maybe_write_pending_text()?;

        // Update the data model.
        for (i, edge) in edges.iter().enumerate() {
            self.edges.insert(edge_index + i, edge.clone());
        }

        // If it's a merge commit, insert a row of '\'.
        if edges.len() >= 2 {
            for edge in self.edges.iter().take(edge_index) {
                AsciiGraphDrawer::straight_edge(&mut self.writer, edge)?;
            }
            AsciiGraphDrawer::straight_edge_no_space(&mut self.writer, &self.edges[edge_index])?;
            for _ in edge_index + 1..self.edges.len() {
                write!(self.writer, "\\ ")?;
            }
            write!(self.writer, " ")?;
            self.maybe_write_pending_text()?;
        }

        let pad_to_index = self.edges.len() + usize::from(edges.is_empty());
        // Close any edges to missing nodes.
        for (i, edge) in edges.iter().enumerate().rev() {
            if *edge == Edge::Missing {
                self.close_missing_edge(edge_index + i, pad_to_index)?;
            }
        }
        // If this was the last node in a chain, add a "/" for any edges to the right of
        // it.
        if edges.is_empty() && edge_index < self.edges.len() {
            self.close_edge(edge_index, pad_to_index)?;
        }

        // Merge new edges that share the same target.
        let mut source_index = 1;
        while source_index < self.edges.len() {
            if let Edge::Present { target, .. } = &self.edges[source_index] {
                if let Some(target_index) = self.index_by_target(target) {
                    // There already is an edge leading to the same target node. Mark that we
                    // want to merge the higher index into the lower index.
                    if source_index > target_index {
                        self.merge_edges(source_index, target_index, pad_to_index)?;
                        // Don't increment source_index.
                        continue;
                    }
                }
            }
            source_index += 1;
        }

        // Emit any remaining lines of text.
        while !self.pending_text.is_empty() {
            for edge in &self.edges {
                AsciiGraphDrawer::straight_edge(&mut self.writer, edge)?;
            }
            for _ in self.edges.len()..pad_to_index {
                write!(self.writer, "  ")?;
            }
            self.maybe_write_pending_text()?;
        }

        Ok(())
    }

    fn default_node_symbol(&self) -> &str {
        "o"
    }

    fn width(&self, id: &K, edges: &[Edge<K>]) -> usize {
        let orig = self.edges.len() - usize::from(self.index_by_target(id).is_some());
        let added = cmp::max(edges.len(), 1);
        2 * (orig + added)
    }
}

impl<'writer, K: Clone + Eq + Hash> AsciiGraphDrawer<'writer, K> {
    fn index_by_target(&self, id: &K) -> Option<usize> {
        for (i, edge) in self.edges.iter().enumerate() {
            match edge {
                Edge::Present { target, .. } if target == id => return Some(i),
                _ => {}
            }
        }
        None
    }

    /// Not an instance method so the caller doesn't need mutable access to the
    /// whole struct.
    fn straight_edge(writer: &mut dyn Write, edge: &Edge<K>) -> io::Result<()> {
        AsciiGraphDrawer::straight_edge_no_space(writer, edge)?;
        write!(writer, " ")
    }

    /// Not an instance method so the caller doesn't need mutable access to the
    /// whole struct.
    fn straight_edge_no_space(writer: &mut dyn Write, edge: &Edge<K>) -> io::Result<()> {
        match edge {
            Edge::Present { direct: true, .. } => {
                write!(writer, "|")?;
            }
            Edge::Present { direct: false, .. } => {
                write!(writer, ":")?;
            }
            Edge::Missing => {
                write!(writer, "|")?;
            }
        }
        Ok(())
    }

    fn merge_edges(&mut self, source: usize, target: usize, pad_to_index: usize) -> io::Result<()> {
        assert!(target < source);
        self.edges.remove(source);
        for i in 0..target {
            AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[i])?;
        }
        if source == target + 1 {
            // If we're merging exactly one step to the left, draw a '/' to join the lines.
            AsciiGraphDrawer::straight_edge_no_space(&mut self.writer, &self.edges[target])?;
            for _ in source..self.edges.len() + 1 {
                write!(self.writer, "/ ")?;
            }
            write!(self.writer, " ")?;
            for _ in self.edges.len() + 1..pad_to_index {
                write!(self.writer, "  ")?;
            }
        } else {
            // If we're merging more than one step to the left, we need two rows:
            // | |_|_|/
            // |/| | |
            AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[target])?;
            for i in target + 1..source - 1 {
                AsciiGraphDrawer::straight_edge_no_space(&mut self.writer, &self.edges[i])?;
                write!(self.writer, "_")?;
            }
            AsciiGraphDrawer::straight_edge_no_space(&mut self.writer, &self.edges[source - 1])?;
            for _ in source..self.edges.len() + 1 {
                write!(self.writer, "/ ")?;
            }
            write!(self.writer, " ")?;
            for _ in self.edges.len() + 1..pad_to_index {
                write!(self.writer, "  ")?;
            }
            self.maybe_write_pending_text()?;

            for i in 0..target {
                AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[i])?;
            }
            AsciiGraphDrawer::straight_edge_no_space(&mut self.writer, &self.edges[target])?;
            write!(self.writer, "/")?;
            for i in target + 1..self.edges.len() {
                AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[i])?;
            }
            for _ in self.edges.len()..pad_to_index {
                write!(self.writer, "  ")?;
            }
        }
        self.maybe_write_pending_text()?;

        Ok(())
    }

    fn close_missing_edge(&mut self, source: usize, pad_to_index: usize) -> io::Result<()> {
        self.edges.remove(source);
        for i in 0..source {
            AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[i])?;
        }
        write!(self.writer, "~")?;
        for _ in source..self.edges.len() {
            write!(self.writer, "/ ")?;
        }
        write!(self.writer, " ")?;
        for _ in self.edges.len() + 1..pad_to_index {
            write!(self.writer, "  ")?;
        }
        self.maybe_write_pending_text()
    }

    fn close_edge(&mut self, source: usize, pad_to_index: usize) -> io::Result<()> {
        for i in 0..source {
            AsciiGraphDrawer::straight_edge(&mut self.writer, &self.edges[i])?;
        }
        write!(self.writer, " ")?;
        for _ in source..self.edges.len() {
            write!(self.writer, "/ ")?;
        }
        write!(self.writer, " ")?;
        for _ in self.edges.len() + 1..pad_to_index {
            write!(self.writer, "  ")?;
        }
        self.maybe_write_pending_text()
    }

    fn maybe_write_pending_text(&mut self) -> io::Result<()> {
        if let Some(text) = self.pending_text.pop() {
            write!(self.writer, "{text}")?;
        }
        writeln!(self.writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adapter to insert width calculation for each `.add_node()`
    struct AsciiGraphDrawerWithWidths<'writer, K> {
        inner: AsciiGraphDrawer<'writer, K>,
    }

    impl<'writer, K: Clone + Eq + Hash> AsciiGraphDrawerWithWidths<'writer, K> {
        fn new(writer: &'writer mut dyn Write) -> Self {
            AsciiGraphDrawerWithWidths {
                inner: AsciiGraphDrawer::new(writer),
            }
        }

        fn add_node(
            &mut self,
            id: &K,
            edges: &[Edge<K>],
            node_symbol: &str,
            text: &str,
        ) -> io::Result<()> {
            let width = self.inner.width(id, edges);
            let text = format!("{text} [width={width}]");
            self.inner.add_node(id, edges, node_symbol, &text)
        }
    }

    #[test]
    fn single_node() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&1, &[], "@", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @ node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn long_description() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawer::new(&mut buffer);
        graph.add_node(&2, &[Edge::direct(1)], "@", "many\nlines\nof\ntext\n")?;
        graph.add_node(&1, &[], "o", "single line")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @ many
        | lines
        | of
        | text
        o single line
        "###);

        Ok(())
    }

    #[test]
    fn long_description_blank_lines() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawer::new(&mut buffer);
        graph.add_node(
            &2,
            &[Edge::direct(1)],
            "@",
            "\n\nmany\n\nlines\n\nof\n\ntext\n\n\n",
        )?;
        graph.add_node(&1, &[], "o", "single line")?;

        // A final newline is ignored but all other newlines are respected.
        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @ 
        | 
        | many
        | 
        | lines
        | 
        | of
        | 
        | text
        | 
        | 
        o single line
        "###);

        Ok(())
    }

    #[test]
    fn chain() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&3, &[Edge::direct(2)], "@", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @ node 3 [width=2]
        o node 2 [width=2]
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn interleaved_chains() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&7, &[Edge::direct(5)], "o", "node 7")?;
        graph.add_node(&6, &[Edge::direct(4)], "o", "node 6")?;
        graph.add_node(&5, &[Edge::direct(3)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(2)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "@", "node 3")?;
        graph.add_node(&2, &[], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 7 [width=2]
        | o node 6 [width=4]
        o | node 5 [width=4]
        | o node 4 [width=4]
        @ | node 3 [width=4]
        | o node 2 [width=4]
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn independent_nodes() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&3, &[Edge::missing()], "o", "node 3")?;
        graph.add_node(&2, &[Edge::missing()], "o", "node 2")?;
        graph.add_node(&1, &[Edge::missing()], "@", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 3 [width=2]
        ~ 
        o node 2 [width=2]
        ~ 
        @ node 1 [width=2]
        ~ 
        "###);

        Ok(())
    }

    #[test]
    fn left_chain_ends() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&4, &[Edge::direct(2)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::missing()], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 4 [width=2]
        | o node 3 [width=4]
        o | node 2 [width=4]
        ~/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn left_chain_ends_with_no_missing_edge() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&4, &[Edge::direct(2)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[], "o", "node 2\nmore\ntext")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 4 [width=2]
        | o node 3 [width=4]
        o | node 2
         /  more
        |   text [width=4]
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn right_chain_ends() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(2)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::missing()], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1\nmore\ntext")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 4 [width=2]
        | o node 3 [width=4]
        | o node 2 [width=4]
        | ~ 
        o node 1
          more
          text [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn right_chain_ends_long_description() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawer::new(&mut buffer);
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(
            &2,
            &[Edge::missing()],
            "o",
            "node 2\nwith\nlong\ndescription",
        )?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 3
        | o node 2
        | ~ with
        |   long
        |   description
        o node 1
        "###);

        Ok(())
    }

    #[test]
    fn fork_multiple() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&4, &[Edge::direct(1)], "@", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @ node 4 [width=2]
        | o node 3 [width=4]
        |/  
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn fork_multiple_chains() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&10, &[Edge::direct(7)], "o", "node 10")?;
        graph.add_node(&9, &[Edge::direct(6)], "o", "node 9")?;
        graph.add_node(&8, &[Edge::direct(5)], "o", "node 8")?;
        graph.add_node(&7, &[Edge::direct(4)], "o", "node 7")?;
        graph.add_node(&6, &[Edge::direct(3)], "o", "node 6")?;
        graph.add_node(&5, &[Edge::direct(2)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 10 [width=2]
        | o node 9 [width=4]
        | | o node 8 [width=6]
        o | | node 7 [width=6]
        | o | node 6 [width=6]
        | | o node 5 [width=6]
        o | | node 4 [width=6]
        | o | node 3 [width=6]
        |/ /  
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn cross_over() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&5, &[Edge::direct(1)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(2)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 5 [width=2]
        | o node 4 [width=4]
        | | o node 3 [width=6]
        | |/  
        |/|   
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn cross_over_multiple() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&7, &[Edge::direct(1)], "o", "node 7")?;
        graph.add_node(&6, &[Edge::direct(3)], "o", "node 6")?;
        graph.add_node(&5, &[Edge::direct(2)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 7 [width=2]
        | o node 6 [width=4]
        | | o node 5 [width=6]
        | | | o node 4 [width=8]
        | |_|/  
        |/| |   
        | o | node 3 [width=6]
        |/ /  
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn cross_over_new_on_left() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&6, &[Edge::direct(3)], "o", "node 6")?;
        graph.add_node(&5, &[Edge::direct(2)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 6 [width=2]
        | o node 5 [width=4]
        | | o node 4 [width=6]
        o | | node 3 [width=6]
        | |/  
        |/|   
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn merge_multiple() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(
            &5,
            &[
                Edge::direct(1),
                Edge::direct(2),
                Edge::direct(3),
                Edge::direct(4),
            ],
            "@",
            "node 5\nmore\ntext",
        )?;
        graph.add_node(&4, &[Edge::missing()], "o", "node 4")?;
        graph.add_node(&3, &[Edge::missing()], "o", "node 3")?;
        graph.add_node(&2, &[Edge::missing()], "o", "node 2")?;
        graph.add_node(&1, &[Edge::missing()], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @---.   node 5
        |\ \ \  more
        | | | | text [width=8]
        | | | o node 4 [width=8]
        | | | ~ 
        | | o node 3 [width=6]
        | | ~ 
        | o node 2 [width=4]
        | ~ 
        o node 1 [width=2]
        ~ 
        "###);

        Ok(())
    }

    #[test]
    fn fork_merge_in_central_edge() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&8, &[Edge::direct(1)], "o", "node 8")?;
        graph.add_node(&7, &[Edge::direct(5)], "o", "node 7")?;
        graph.add_node(
            &6,
            &[Edge::direct(2)],
            "o",
            "node 6\nwith\nsome\nmore\nlines",
        )?;
        graph.add_node(&5, &[Edge::direct(4), Edge::direct(3)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 8 [width=2]
        | o node 7 [width=4]
        | | o node 6
        | | | with
        | | | some
        | | | more
        | | | lines [width=6]
        | o |   node 5 [width=8]
        | |\ \  
        | o | | node 4 [width=8]
        |/ / /  
        | o | node 3 [width=6]
        |/ /  
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn fork_merge_multiple() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&6, &[Edge::direct(5)], "o", "node 6")?;
        graph.add_node(
            &5,
            &[Edge::direct(2), Edge::direct(3), Edge::direct(4)],
            "o",
            "node 5",
        )?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 6 [width=2]
        o-.   node 5 [width=6]
        |\ \  
        | | o node 4 [width=6]
        | o | node 3 [width=6]
        | |/  
        o | node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn fork_merge_multiple_in_central_edge() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&10, &[Edge::direct(1)], "o", "node 10")?;
        graph.add_node(&9, &[Edge::direct(7)], "o", "node 9")?;
        graph.add_node(&8, &[Edge::direct(2)], "o", "node 8")?;
        graph.add_node(
            &7,
            &[
                Edge::direct(6),
                Edge::direct(5),
                Edge::direct(4),
                Edge::direct(3),
            ],
            "o",
            "node 7",
        )?;
        graph.add_node(&6, &[Edge::direct(1)], "o", "node 6")?;
        graph.add_node(&5, &[Edge::direct(1)], "o", "node 5")?;
        graph.add_node(&4, &[Edge::direct(1)], "o", "node 4")?;
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(&2, &[Edge::direct(1)], "o", "node 2")?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 10 [width=2]
        | o node 9 [width=4]
        | | o node 8 [width=6]
        | |  \
        | |    \
        | o---. |   node 7 [width=12]
        | |\ \ \ \  
        | o | | | | node 6 [width=12]
        |/ / / / /  
        | o | | | node 5 [width=10]
        |/ / / /  
        | o | | node 4 [width=8]
        |/ / /  
        | o | node 3 [width=6]
        |/ /  
        | o node 2 [width=4]
        |/  
        o node 1 [width=2]
        "###);

        Ok(())
    }

    #[test]
    fn merge_multiple_missing_edges() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(
            &1,
            &[
                Edge::missing(),
                Edge::missing(),
                Edge::missing(),
                Edge::missing(),
            ],
            "@",
            "node 1\nwith\nmany\nlines\nof\ntext",
        )?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        @---.   node 1
        |\ \ \  with
        | | | ~ many
        | | ~   lines
        | ~     of
        ~       text [width=8]
        "###);

        Ok(())
    }

    #[test]
    fn merge_missing_edges_and_fork() -> io::Result<()> {
        let mut buffer = vec![];
        let mut graph = AsciiGraphDrawerWithWidths::new(&mut buffer);
        graph.add_node(&3, &[Edge::direct(1)], "o", "node 3")?;
        graph.add_node(
            &2,
            &[
                Edge::missing(),
                Edge::indirect(1),
                Edge::missing(),
                Edge::indirect(1),
            ],
            "o",
            "node 2\nwith\nmany\nlines\nof\ntext",
        )?;
        graph.add_node(&1, &[], "o", "node 1")?;

        insta::assert_snapshot!(String::from_utf8_lossy(&buffer), @r###"
        o node 3 [width=2]
        | o---.   node 2
        | |\ \ \  with
        | | : ~/  many
        | ~/ /    lines
        |/ /      of
        |/        text [width=10]
        o node 1 [width=2]
        "###);

        Ok(())
    }
}
