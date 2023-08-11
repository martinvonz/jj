# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")

def normalise_metadata(data: [str, list[str]]) -> [cmd_args, list[cmd_args]]:
    if type(data) == type([]):
        return [cmd_args(item) for item in data]
    else:
        return cmd_args(data)

def to_term_args(data: "") -> cmd_args:
    """ convert nested lists/tuple/map data structure to Erlang Term cmd_args
    """
    args = cmd_args([])
    args.add(cmd_args([
        convert(data),
        ".",
    ], delimiter = ""))
    args.add("")
    return args

# paths
def app_file(ctx: AnalysisContext) -> str:
    return paths.join(beam_dir(ctx), ctx.attrs.name + ".app")

def beam_dir(ctx: AnalysisContext) -> str:
    return paths.join(ctx.attrs.name, "ebin")

def beam_path(ctx: AnalysisContext, src: "artifact") -> str:
    return paths.join(beam_dir(ctx), paths.replace_extension(src.basename, ".beam"))

def linktree() -> str:
    return "linktree"

build_paths = struct(
    app_file = app_file,
    beam_dir = beam_dir,
    beam_path = beam_path,
    linktree = linktree,
)

def convert(data: "") -> cmd_args:
    """ converts a lists/tuple/map data structure to a sub-term that can be embedded in another to_term_args or convert
    """
    if type(data) == "list":
        return convert_list(data)
    elif type(data) == "tuple":
        return convert_list(list(data), ob = "{", cb = "}")
    elif type(data) == "dict":
        return convert_dict(data)
    elif type(data) == "string":
        return convert_string(data)
    elif type(data) == "cmd_args":
        return data
    elif type(data) == "bool":
        return convert_bool(data)

    args = cmd_args([])
    args.add(cmd_args(["\"", data, "\""], delimiter = ""))
    return args

# internal
def convert_list(ls: list, ob: str = "[", cb: str = "]") -> cmd_args:
    args = cmd_args([])
    args.add(ob)
    if len(ls) >= 1:
        args.add(cmd_args([
            convert(ls[0]),
        ], delimiter = ""))
        for item in ls[1:]:
            args.add(cmd_args([
                ",",
                convert(item),
            ], delimiter = ""))
    args.add(cb)
    return args

def convert_dict(dt: dict) -> cmd_args:
    args = cmd_args([])
    args.add("#{")
    items = list(dt.items())
    if len(items) >= 1:
        k, v = items[0]
        args.add(cmd_args([
            convert(k),
            "=>",
            convert(v),
        ], delimiter = ""))
        for k, v in items[1:]:
            args.add(cmd_args([
                ",",
                convert(k),
                "=>",
                convert(v),
            ], delimiter = ""))
    args.add("}")
    return args

def convert_args(data: cmd_args) -> cmd_args:
    args = cmd_args()
    args.add("\"")
    args.add(cmd_args(data, delimiter = " "))
    args.add("\"")
    return args

def convert_string(st: str) -> cmd_args:
    args = cmd_args()
    return args.add(cmd_args(["\"", st.replace("\"", "\\\""), "\""], delimiter = ""))

def convert_bool(bl: bool) -> cmd_args:
    if bl:
        return cmd_args(["true"])
    else:
        return cmd_args(["false"])

def multidict_projection(build_environments: dict[str, "BuildEnvironment"], field_name: str) -> dict:
    field = {}
    for name, env in build_environments.items():
        field[name] = getattr(env, field_name)
    return field

def multidict_projection_key(build_environments: dict[str, "BuildEnvironment"], field_name: str, key: str) -> dict:
    field = {}
    for name, env in build_environments.items():
        dict_val = getattr(env, field_name)
        field[name] = dict_val[key]
    return field

def action_identifier(toolchain: "Toolchain", name: str) -> str:
    """builds an action identifier parameterized by the toolchain"""
    return "%s(%s)" % (name, toolchain.name)

def str_to_bool(value: str) -> bool:
    """convert string representation of bool to bool"""
    if value == "True":
        return True
    elif value == "False":
        return False
    else:
        fail("{} is not a valid boolean value")

def preserve_structure(path: str) -> dict[str, list[str]]:
    """Return a mapping from a path that preserves the filestructure relative to the path."""
    all_files = glob([paths.join(path, "**")])
    mapping = {}
    for filename in all_files:
        relative_path = paths.relativize(filename, path)
        dirname = paths.dirname(relative_path)
        mapping[dirname] = mapping.get(dirname, []) + [filename]
    return mapping

def _file_mapping_impl(ctx: AnalysisContext) -> list["provider"]:
    outputs = []
    for target_path, files in ctx.attrs.mapping.items():
        for file in files:
            target_path = paths.normalize(target_path)
            out_path = paths.normalize(paths.join(target_path, file.basename))
            out = ctx.actions.copy_file(
                out_path,
                file,
            )
            outputs.append(out)

    return [DefaultInfo(default_outputs = outputs)]

def list_dedupe(xs: list[str]) -> list[str]:
    return {x: True for x in xs}.keys()

file_mapping = rule(
    impl = _file_mapping_impl,
    attrs = {
        "mapping": attrs.dict(key = attrs.string(), value = attrs.list(attrs.source()), default = {}),
    },
)
