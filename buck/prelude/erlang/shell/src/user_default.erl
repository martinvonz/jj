%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format
-module(user_default).
-wacov(ignore).

-export([
    c/1, c/2, c/3,
    l/1
]).

-type c_ret() :: {ok, module()} | error.
-type c_options() :: [compile:option()] | compile:option().

-spec c(module()) -> c_ret().
c(Module) ->
    c(Module, []).

-spec c(module(), c_options()) -> c_ret().
c(Module, Options) ->
    c(Module, Options, fun(_) -> true end).

-spec c(module(), c_options(), fun((compile:option()) -> boolean())) -> c_ret().
c(Module, _Options, _Filter) ->
    case code:which(Module) of
        non_existing ->
            {error, non_existing};
        _ ->
            case shell_buck2_utils:rebuild_modules([Module]) of
                ok ->
                    ok = ct_daemon:push_module(Module),
                    code:purge(Module),
                    code:load_file(Module),
                    {ok, Module};
                error ->
                    error
            end
    end.

-spec l(module()) -> code:load_ret().
l(Module) ->
    case find_module(Module) of
        available ->
            c:l(Module);
        {source, RelSource} ->
            Paths = shell_buck2_utils:get_additional_paths(RelSource),
            ok = code:add_paths(Paths),
            ok = ct_daemon:push_paths(Paths),
            c:l(Module);
        Error ->
            Error
    end.

-spec find_module(module()) ->
    available
    | {source, file:filename_all()}
    | {error, not_found | {ambiguous, [file:filename_all()]}}.
find_module(Module) ->
    WantedModuleName = atom_to_list(Module),
    case
        [
            found
         || {ModuleName, _, _} <- code:all_available(),
            string:equal(WantedModuleName, ModuleName)
        ]
    of
        [found] -> available;
        _ -> find_module_source(Module)
    end.

-spec find_module_source(module()) ->
    {source, file:filename_all()}
    | {error, not_found | {ambiguous, [file:filename_all()]}}.
find_module_source(Module) ->
    Root = shell_buck2_utils:project_root(),
    {ok, Output} = shell_buck2_utils:run_command(
        "find ~s -type d "
        "\\( -path \"~s/_build*\" -path \"~s/erl/_build*\" -o -path ~s/buck-out \\) -prune "
        "-o -name '~s.erl' -print",
        [Root, Root, Root, Root, Module]
    ),
    case
        [
            RelPath
         || RelPath <- [
                string:prefix(Path, [Root, "/"])
             || Path <- string:split(Output, "\n", all)
            ],
            RelPath =/= nomatch,
            string:prefix(RelPath, "buck-out") == nomatch,
            string:str(binary_to_list(RelPath), "_build") == 0
        ]
    of
        [ModulePath] ->
            {source, ModulePath};
        [] ->
            {error, not_found};
        Candidates ->
            %% check if there are actually targets associated
            {ok, RawOutput} = shell_buck2_utils:buck2_query(
                "owner(\\\"\%s\\\")", "--json", Candidates
            ),
            SourceTargetMapping = jsone:decode(RawOutput),
            case
                maps:fold(
                    fun
                        (_Source, [], Acc) -> Acc;
                        (Source, _, Acc) -> [Source | Acc]
                    end,
                    [],
                    SourceTargetMapping
                )
            of
                [] -> {error, not_found};
                [Source] -> {source, Source};
                More -> {error, {ambiguous, More}}
            end
    end.
