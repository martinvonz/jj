# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "expect")

def topo_sort(graph: dict["_a", list["_a"]]) -> list["_a"]:
    """
    Topo-sort the given graph.
    """

    in_degrees = {node: 0 for node in graph}
    for _node, deps in graph.items():
        for dep in dedupe(deps):
            in_degrees[dep] += 1

    queue = []

    for node, in_degree in in_degrees.items():
        if in_degree == 0:
            queue.append(node)

    ordered = []

    for _ in range(len(in_degrees)):
        expect(len(queue) != 0, "cycle in graph detected")

        node = queue.pop()
        ordered.append(node)

        for dep in graph[node]:
            in_degrees[dep] -= 1
            if in_degrees[dep] == 0:
                queue.append(dep)

    expect(not queue, "finished before processing nodes: {}".format(queue))
    expect(len(ordered) == len(graph), "missing or duplicate nodes in sort")

    return ordered

def post_order_traversal(graph: dict["_a", list["_a"]]) -> list["_a"]:
    """
    Performs a post-order traversal of `graph`.
    """

    out_degrees = {node: 0 for node in graph}
    rdeps = {node: [] for node in graph}
    for node, deps in graph.items():
        for dep in dedupe(deps):
            out_degrees[node] += 1
            rdeps[dep].append(node)

    queue = []

    for node, out_degree in out_degrees.items():
        if out_degree == 0:
            queue.append(node)

    ordered = []

    for _ in range(len(out_degrees)):
        expect(len(queue) != 0, "cycle in graph detected")

        node = queue.pop()
        ordered.append(node)

        for dep in rdeps[node]:
            out_degrees[dep] -= 1
            if out_degrees[dep] == 0:
                queue.append(dep)

    expect(not queue, "finished before processing nodes: {}".format(queue))
    expect(len(ordered) == len(graph), "missing or duplicate nodes in sort")

    return ordered

def breadth_first_traversal(
        graph_nodes: dict["_a", list["_a"]],
        roots: list["_a"]) -> list["_a"]:
    """
    Like `breadth_first_traversal_by` but the nodes are stored in the graph.
    """

    def lookup(x):
        return graph_nodes[x]

    return breadth_first_traversal_by(graph_nodes, roots, lookup)

def breadth_first_traversal_by(
        graph_nodes: dict["_a", ""],
        roots: list["_a"],
        get_nodes_to_traverse_func) -> list["_a"]:
    """
    Performs a breadth first traversal of `graph_nodes`, beginning
    with the `roots` and queuing the nodes returned by`get_nodes_to_traverse_func`.
    Returns a list of all visisted nodes.

    get_nodes_to_traverse_func(node: '_a') -> ['_a']:

    Starlark does not offer while loops, so this implementation
    must make use of a for loop. We pop from the end of the queue
    as a matter of performance.
    """

    # Dictify for O(1) lookup
    visited = {k: None for k in roots}

    queue = visited.keys()

    for _ in range(len(graph_nodes)):
        if not queue:
            break
        node = queue.pop()
        expect(node in graph_nodes, "Expected node {} in graph nodes", node)
        nodes_to_visit = get_nodes_to_traverse_func(node)
        for node in nodes_to_visit:
            if node not in visited:
                visited[node] = None
                queue.append(node)

    expect(not queue, "Expected to be done with graph traversal queue.")

    return visited.keys()
