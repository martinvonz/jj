%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Search in the execution directory produced by buck2 test
%%% for relevant logs to display to the user.
%%% Link them into a temporary directory, and produce a json output
%%% that lists them.
%%% @end
%%% % @format

-module(execution_logs).

-compile(warn_missing_spec).

%% Public API
-export([create_dir_summary/1]).

-type key() ::
    buck2_exec_dir | log_private | suite_html | scuba_link | test_log_json | ct_log | ct_stdout.

-type key_entry() :: {key(), string()} | not_found.

-spec create_dir_summary(file:filename()) -> #{atom() => binary()}.
create_dir_summary(ExecDir) ->
    TempDir = create_temp_directory(),
    Funcs = [
        fun add_test_log/2,
        fun add_test_log_json/2,
        fun add_suite_html/2,
        fun add_log_private/2,
        fun add_exec_dir/2,
        fun add_ct_log/2,
        fun add_ct_stdout/2
    ],
    lists:foldl(
        fun(Func, Map) ->
            case Func(TempDir, ExecDir) of
                not_found -> Map;
                {Key, Path} -> Map#{Key => list_to_binary(Path)}
            end
        end,
        #{},
        Funcs
    ).

-spec add_ct_log(file:filename(), file:filename()) -> key_entry().
add_ct_log(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "ct_executor.log", file) of
        {error, _} ->
            not_found;
        TestLogJson ->
            file:make_symlink(TestLogJson, filename:join(TempDir, "ct.log")),
            {ct_log, filename:join(TempDir, "ct.log")}
    end.

-spec add_ct_stdout(file:filename(), file:filename()) -> key_entry().
add_ct_stdout(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "ct_executor.stdout.txt", file) of
        {error, _} ->
            not_found;
        TestLogJson ->
            file:make_symlink(TestLogJson, filename:join(TempDir, "ct.stdout")),
            {ct_stdout, filename:join(TempDir, "ct.stdout")}
    end.

-spec add_test_log(file:filename(), file:filename()) -> key_entry().
add_test_log(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "**/test.log", file) of
        {error, _} ->
            not_found;
        TestLogJson ->
            file:make_symlink(TestLogJson, filename:join(TempDir, "test.log")),
            {test_log, filename:join(TempDir, "test.log")}
    end.

-spec add_test_log_json(file:filename(), file:filename()) -> key_entry().
add_test_log_json(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "**/test.log.json", file) of
        {error, _} ->
            not_found;
        TestLogJson ->
            file:make_symlink(TestLogJson, filename:join(TempDir, "test.log.json")),
            {test_log_json, filename:join(TempDir, "test.log.json")}
    end.

-spec add_suite_html(file:filename(), file:filename()) -> key_entry().
add_suite_html(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "**/suite.log.html", file) of
        {error, _} ->
            not_found;
        SuiteHtml ->
            file:make_symlink(filename:dirname(SuiteHtml), filename:join(TempDir, "htmls")),
            {suite_html, filename:join([TempDir, "htmls", "suite.log.html"])}
    end.

-spec add_log_private(file:filename(), file:filename()) -> key_entry().
add_log_private(TempDir, ExecDir) ->
    case find_pattern(ExecDir, "**/log_private", folder) of
        {error, _} ->
            not_found;
        LogPrivate ->
            file:make_symlink(LogPrivate, filename:join(TempDir, "log_private")),
            {log_private, filename:join(TempDir, "log_private")}
    end.

-spec add_exec_dir(file:filename(), file:filename()) -> key_entry().
add_exec_dir(TempDir, ExecDir) ->
    file:make_symlink(ExecDir, filename:join(TempDir, "exec_dir")),
    {buck2_exec_dir, filename:join(TempDir, "exec_dir")}.

-spec create_temp_directory() -> file:filename().
create_temp_directory() ->
    RootTmpDir =
        case os:getenv("TEMPDIR") of
            false ->
                NewTmpDir = os:cmd("mktemp"),
                filename:dirname(NewTmpDir);
            Dir ->
                Dir
        end,
    {_, _, Micro} = TS = os:timestamp(),
    {{_Year, Month, Day}, {Hour, Minute, Second}} = calendar:now_to_universal_time(TS),
    DateTime = unicode:characters_to_list(
        io_lib:format("~2..0w.~2..0wT~2..0w.~2..0w.~2..0w.~w", [
            Month, Day, Hour, Minute, Second, Micro
        ])
    ),
    is_list(DateTime) orelse error(uncode_format_failed, DateTime),
    TmpDir = filename:join([RootTmpDir, "buck2_test_logs", DateTime]),
    filelib:ensure_path(TmpDir),
    TmpDir.

-spec find_pattern(file:filename(), file:filename(), file | folder) ->
    {error, not_found} | file:filename().
find_pattern(ExecDir, Pattern, FolderOrFile) ->
    Func =
        case FolderOrFile of
            folder -> fun filelib:is_dir/1;
            file -> fun filelib:is_regular/1
        end,
    Candidates = [
        Path
     || Path <- filelib:wildcard(filename:join(ExecDir, Pattern)), Func(Path)
    ],
    case Candidates of
        [] -> {error, not_found};
        [LogPrivate | _Tail] -> LogPrivate
    end.
