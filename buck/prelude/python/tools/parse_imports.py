#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import ast
import json
import logging
import os
import warnings
from pathlib import Path
from typing import Dict, FrozenSet, List, Optional, Set, Tuple, Union

from py38stdlib import STDLIB_MODULES


logger = logging.getLogger(__name__)  # type: logging.Logger
COMMON_SPECIAL_PYTHON_SYMBOLS = {
    "__builtins__",
    "__doc__",
    "__file__",
    "__loader__",
    "__name__",
    "__package__",
    "__spec__",
}


class ImportVisitor(ast.NodeVisitor):
    IMPORT_FUNCTIONS = {"lazy_import", "import_module", "__import__"}
    STAR_IMPORT = "*"
    stdlib: FrozenSet[str] = STDLIB_MODULES

    def __init__(
        self,
        base_module: str,
    ) -> None:
        self.base_module: str = base_module
        self.base_module_chunks: List[str] = base_module.split(".")
        self.modules: Dict[str, int] = {}

    def get_imported_modules(self, node: ast.Module) -> Dict[str, int]:
        self.modules = {}
        self.visit(node)
        return self.modules

    def _add(self, module: str, lineno: int) -> None:
        path = module.split(".")
        if path[0] in self.stdlib:
            return
        # Ignore __manifest__ file which are generated
        elif any(chunk == "__manifest__" for chunk in path):
            return
        self.modules[module] = lineno

    def visit_If(self, node: ast.If) -> None:
        for stmt in node.orelse:
            self.visit(stmt)

        for stmt in node.body:
            self.visit(stmt)

    def visit_Import(self, node: ast.Import) -> None:
        for name in node.names:
            self._add(name.name, node.lineno)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.level == 0 and node.module is not None:
            chunks = [node.module]
        else:
            chunks = self.base_module_chunks
            chunks = chunks[: len(chunks) - node.level + 1]

            if node.module is not None:
                chunks.append(node.module)

        # we only care about the module
        # self._add(".".join(chunks), node.lineno)
        for name in node.names:
            if name.name == self.STAR_IMPORT:
                # Just add module in case of star imports
                self._add(".".join(chunks), node.lineno)
            else:
                self._add(".".join([*chunks, name.name]), node.lineno)

    def visit_Call(self, node: ast.Call) -> None:
        try:
            # pyre-fixme[16]: `expr` has no attribute `id`.
            if node.func.id not in self.IMPORT_FUNCTIONS:  # noqa: T484
                return
            if len(node.args) == 2:
                # pyre-fixme[16]: `expr` has no attribute `s`.
                self._add(f"{node.args[0].s}.{node.args[1].s}", node.lineno)
            else:
                self._add(node.args[0].s, node.lineno)
        except AttributeError:
            return


class TopSymbolsVisitor(ast.NodeVisitor):
    def __init__(self, path: str) -> None:
        self.path = path
        self.star_imports: set[str] = set()
        self.symbols: set[str] = set()
        self._top_level: set[ast.stmt] = set()

    def visit(self, node: ast.AST) -> None:
        if not isinstance(node, ast.Module) and node not in self._top_level:
            return
        try:
            return super().visit(node)
        except AttributeError as exc:
            logger.error(
                "Got %r when parsing %s from %s", exc, ast.dump(node), self.path
            )

    def visit_Module(self, node: ast.Module) -> None:
        self._top_level = set(node.body)
        return self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module and [n.name for n in node.names] == ["*"]:
            self.star_imports.add(node.module)
        else:
            self.symbols.update({name.asname or name.name for name in node.names})
        return self.generic_visit(node)

    def visit_Import(self, node: ast.Import) -> None:
        self.symbols.update(
            name.asname for name in node.names if name.asname is not None
        )
        return self.generic_visit(node)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.symbols.add(node.name)
        return self.generic_visit(node)

    # pyre-fixme[4]: Missing attribute annotation
    visit_AsyncFunctionDef = visit_ClassDef = visit_FunctionDef

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        # pyre-fixme[16]: `expr` has no attribute `id`.
        self.symbols.add(node.target.id)  # noqa: T484
        return self.generic_visit(node)

    def visit_Assign(self, node: ast.Assign) -> None:
        for target in node.targets:
            self._inspect_target(target)
        return self.generic_visit(node)

    def visit_With(self, node: ast.With) -> None:
        self._top_level.update(node.body)
        return self.generic_visit(node)

    def visit_If(self, node: ast.If) -> None:
        self._top_level.update(node.body)
        self._top_level.update(node.orelse)
        return self.generic_visit(node)

    def visit_Try(self, node: ast.Try) -> None:
        self._top_level.update(node.body)
        # pyre-fixme[6]: expected `Iterable[stmt]` but got `List[ExceptHandler]`
        self._top_level.update(node.handlers)
        self._top_level.update(node.orelse)
        self._top_level.update(node.finalbody)
        return self.generic_visit(node)

    # pyre-fixme[2]: `target` has no type specified
    def _inspect_target(self, target) -> None:
        if isinstance(target, (ast.Subscript, ast.Attribute)):
            return
        elif isinstance(target, (ast.Tuple, ast.List)):
            for elt in target.elts:
                self._inspect_target(elt)
        elif isinstance(target, (ast.Attribute, ast.Starred)):
            while not isinstance(getattr(target, "value", None), ast.Name):
                target = target.value
            self.symbols.add(target.value.id)
        else:
            self.symbols.add(target.id)


def get_top_level_symbols(
    file: Path, content: Union[str, bytes]
) -> Tuple[Set[str], Set[str]]:
    path = str(file)
    module = _get_ast_tree(path, content)

    if module is None:
        return (set(), set())

    visitor = TopSymbolsVisitor(path)
    visitor.visit(module)

    return (visitor.symbols | COMMON_SPECIAL_PYTHON_SYMBOLS, visitor.star_imports)


def get_full_symbols(module_symbols: Dict[str, Set[str]]) -> List[str]:
    ret = set()
    for module, symbols in module_symbols.items():
        ret.add(module)
        ret.update(f"{module}.{symbol}" for symbol in symbols)

    return sorted(ret)


def module_from_src(src: str, prefix: str) -> str:
    init_suffix = "__init__.py"

    if src.endswith(init_suffix):
        ret = src[: -len(init_suffix)]
    else:
        ret = os.path.splitext(src)[0]

    return f"{prefix}{ret}".replace("/", ".").rstrip(".")


def update_symbols_with_star_imports(
    symbols: Dict[str, Set[str]], star_imports: Dict[str, Set[str]]
) -> None:
    """Resolve star imports, changing the contents of the symbols map"""

    # memoization cache, but don't create a new dict/set of all symbols
    completed: Set[str] = set()

    def resolve_one(current: str) -> None:
        if current in completed:
            return

        completed.add(current)
        for other in star_imports.get(current, set()):
            if other not in symbols:
                continue
            resolve_one(other)
            symbols[current].update(symbols[other])

    for module in star_imports.keys():
        resolve_one(module)


def _get_ast_tree(path: str, content: Union[str, bytes]) -> Optional[ast.Module]:
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            return ast.parse(content, filename=path)
    except SyntaxError:
        return None


def main() -> int:
    """Extract imports modules from a python source file"""
    parser = argparse.ArgumentParser()
    parser.add_argument("file", help="the python source file to be read")
    parser.add_argument("out", help="the file write data to")
    args = parser.parse_args()

    with open(args.file, encoding="utf-8-sig") as f:
        try:
            root = ast.parse(f.read(), filename=args.file)
            visitor = ImportVisitor(base_module=args.file.replace("/", "."))
            modules_list = list(visitor.get_imported_modules(root).keys())
        except UnicodeDecodeError:
            print(f"Can't parse: {args.file}")
            modules_list = []
        except RecursionError:
            print(f"Recursion depth reached traversing ast: {args.file}")
            modules_list = []

    with open(args.out, "w") as f:
        f.write(json.dumps({"modules": modules_list}))
    return 0


if __name__ == "__main__":
    main()
