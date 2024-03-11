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
use std::io;
use std::io::Write;

use itertools::Itertools;
use jj_lib::settings::UserSettings;
use renderdag::{Ancestor, GraphRowRenderer, Renderer};

#[derive(Debug, Clone, PartialEq, Eq)]
// An edge to another node in the graph
pub enum Edge<T> {
    Direct(T),
    Indirect(T),
    Missing,
}

pub trait GraphLog<K: Clone + Eq + Hash> {
    fn add_node(
        &mut self,
        id: &K,
        edges: &[Edge<K>],
        node_symbol: &str,
        text: &str,
    ) -> io::Result<()>;

    fn width(&self, id: &K, edges: &[Edge<K>]) -> usize;
}

pub struct SaplingGraphLog<'writer, R> {
    renderer: R,
    writer: &'writer mut dyn Write,
}

impl<K: Clone> From<&Edge<K>> for Ancestor<K> {
    fn from(e: &Edge<K>) -> Self {
        match e {
            Edge::Direct(target) => Ancestor::Parent(target.clone()),
            Edge::Indirect(target) => Ancestor::Ancestor(target.clone()),
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
    ) -> Box<dyn GraphLog<K> + 'writer>
    where
        K: Clone + Eq + Hash + 'writer,
        R: Renderer<K, Output = String> + 'writer,
    {
        Box::new(SaplingGraphLog {
            renderer,
            writer: formatter,
        })
    }
}

pub fn get_graphlog<'a, K: Clone + Eq + Hash + 'a>(
    settings: &UserSettings,
    formatter: &'a mut dyn Write,
) -> Box<dyn GraphLog<K> + 'a> {
    let builder = GraphRowRenderer::new().output().with_min_row_height(0);
    match settings.graph_style().as_str() {
        "square" => {
            SaplingGraphLog::create(builder.build_box_drawing().with_square_glyphs(), formatter)
        }
        "ascii" => SaplingGraphLog::create(builder.build_ascii(), formatter),
        "ascii-large" => SaplingGraphLog::create(builder.build_ascii_large(), formatter),
        // "curved"
        _ => SaplingGraphLog::create(builder.build_box_drawing(), formatter),
    }
}
