%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Documentation for shell_buck2_utils, ways to use
%%%   it, ways to break it, etc. etc
%%% @end
%%% % @format

-module(shell_buck2_utils).

%% Public API
-export([
    project_root/0,
    rebuild_modules/1,
    buck2_build_targets/1,
    buck2_query/1, buck2_query/2, buck2_query/3,
    run_command/2,
    get_additional_paths/1
]).

-type opt() :: {at_root, boolean()} | {replay, boolean()}.

-spec project_root() -> file:filename().
project_root() ->
    case run_command("buck2 root --kind=project 2>/dev/null", [], [{at_root, false}, {replay, false}]) of
        {ok, Output} ->
            Dir = string:trim(Output),
            case filelib:is_dir(Dir) of
                true -> Dir;
                false -> error({project_root_not_found, Dir})
            end;
        error ->
            error(failed_to_query_project_root)
    end.

-spec project_cell() -> binary().
project_cell() ->
    ProjectRoot = project_root(),
    case run_command("buck2 audit cell --json 2>/dev/null", [], [{replay, false}]) of
        {ok, Output} ->
            [ProjectCell] = [
                Cell
             || {Cell, CellRoot} <- maps:to_list(jsone:decode(Output)), string:equal(ProjectRoot, CellRoot)
            ],
            ProjectCell;
        error ->
            error(failed_to_query_project_cell)
    end.

-spec rebuild_modules([module()]) -> ok | error.
rebuild_modules([]) ->
    ok;
rebuild_modules(Modules) ->
    case lists:filter(fun(Module) -> code:which(Module) == non_existing end, Modules) of
        [] -> ok;
        Missing -> error({non_existing, Missing})
    end,
    RelSources = [proplists:get_value(source, Module:module_info(compile)) || Module <- Modules],
    {ok, RawQueryResult} = buck2_query("owner(\%s)", RelSources),
    Targets = string:split(string:trim(RawQueryResult), "\n", all),
    case Targets of
        [] ->
            io:format("ERROR: couldn't find targets for ~w~n", [Modules]),
            error;
        _ ->
            buck2_build_targets(Targets)
    end.

-spec buck2_build_targets([string() | binary()]) -> ok | error.
buck2_build_targets(Targets) ->
    case
        run_command("buck2 build --reuse-current-config --console super ~s", [
            lists:join(" ", Targets)
        ])
    of
        {ok, _Output} -> ok;
        error -> error
    end.

-spec buck2_query(string()) -> {ok, binary()} | error.
buck2_query(Query) ->
    buck2_query(Query, []).

-spec buck2_query(string(), [string()]) -> {ok, binary()} | error.
buck2_query(Query, Args) ->
    buck2_query(Query, "", Args).

-spec buck2_query(string(), string(), [string()]) -> {ok, binary()} | error.
buck2_query(Query, BuckArgs, Args) ->
    run_command("buck2 uquery ~s --reuse-current-config \"~s\" ~s 2> /dev/null", [
        BuckArgs, Query, lists:join(" ", Args)
    ]).

-spec run_command(string(), [term()]) -> {ok, binary()} | error.
run_command(Fmt, Args) ->
    run_command(Fmt, Args, []).

-spec run_command(string(), [term()], [opt()]) -> {ok, binary()} | error.
run_command(Fmt, Args, Options) ->
    PortOpts0 = [exit_status, stderr_to_stdout],
    PortOpts1 =
        case proplists:get_value(at_root, Options, true) of
            true ->
                Root = project_root(),
                [{cd, Root} | PortOpts0];
            false ->
                PortOpts0
        end,

    RawCmd = io_lib:format(Fmt, Args),
    Cmd = unicode:characters_to_list(RawCmd),

    Replay = proplists:get_value(replay, Options, true),

    Port = erlang:open_port({spawn, Cmd}, PortOpts1),
    port_loop(Port, Replay, []).

-spec port_loop(port(), boolean(), [binary()]) -> {ok, binary()} | error.
port_loop(Port, Replay, StdOut) ->
    receive
        {Port, {exit_status, 0}} ->
            {ok, unicode:characters_to_binary(lists:reverse(StdOut))};
        {Port, {exit_status, _}} ->
            error;
        {Port, {data, Data}} ->
            case Replay of
                true -> io:put_chars(Data);
                false -> ok
            end,
            port_loop(Port, Replay, [Data | StdOut])
    end.

-spec get_additional_paths(file:filename_all()) -> [file:filename_all()].
get_additional_paths(Path) ->
    PrefixedPath = io_lib:format("~s//~s", [project_cell(), Path]),
    case
        run_command(
            "buck2 bxl --reuse-current-config --console super prelude//erlang/shell/shell.bxl:ebin_paths -- --source ~s",
            [
                PrefixedPath
            ]
        )
    of
        {ok, Output} ->
            MaybeOutputPaths = [
                filter_escape_chars(OutputPath)
             || OutputPath <- string:split(Output, "\n", all)
            ],
            MaybeAllPaths = lists:concat([
                [OutputPath, filename:join(OutputPath, "ebin")]
             || OutputPath <- MaybeOutputPaths, filelib:is_dir(OutputPath)
            ]),
            [MaybePath || MaybePath <- MaybeAllPaths, filelib:is_dir(MaybePath)];
        error ->
            []
    end.

%% copied from stackoverflow: https://stackoverflow.com/questions/14693701/how-can-i-remove-the-ansi-escape-sequences-from-a-string-in-python
-define(ANSI_ESCAPE_REGEX,
    "(\x9B|\x1B\\[)[0-?]*[ -/]*[@-~]"
).

filter_escape_chars(String) ->
    lists:flatten(io_lib:format("~s", [re:replace(String, ?ANSI_ESCAPE_REGEX, "", [global])])).
