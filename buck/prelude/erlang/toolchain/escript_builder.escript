%% #!/usr/bin/env escript
%% -*- erlang -*-
%%! +sbtu

%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc
%%%  Build an escript from a given spec file. The spec file format
%%%  is defined in erlang_escript.bzl
%%%
%%%  usage:
%%%    escript_builder.escript escript_build_spec.term
%%% @end

-module(app_src_builder).
-author("loscher@fb.com").

-export([main/1]).

-include_lib("kernel/include/file.hrl").

-mode(compile).

-type escript_artifact_spec() :: #{
    ArchivePath :: file:filename() => FileSystemPath :: file:filename()
}.
-type escript_load_spec() :: [{ArchivePath :: file:filename(), FileSystemPath :: file:filename()}].
-type escript_archive_spec() :: [{ArchivePath :: file:filename(), binary()}].

-spec main([string()]) -> ok.
main([Spec]) ->
    try
        io:format("~p ~p~n", [Spec, file:consult(Spec)]),
        {ok, [Terms]} = file:consult(Spec),
        do(Terms)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~s: ~s~n", [Type, Reason]),
            erlang:halt(1)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("escript_builder.escript build_spec.term ~n").

-spec do(#{}) -> ok.
do(#{
    "artifacts" := Artifacts,
    "emu_args" := EmuArgs0,
    "output" := EscriptPath
}) ->
    ArchiveSpec = prepare_files(Artifacts),
    Shebang = "/usr/bin/env escript",
    Comment = "",
    EmuArgs1 = [string:strip(Arg) || Arg <- EmuArgs0],
    FinalEmuArgs = unicode:characters_to_list(
        [" ", string:join(EmuArgs1, " ")]
    ),
    EscriptSections =
        [
            {shebang, Shebang},
            {comment, Comment},
            {emu_args, FinalEmuArgs},
            {archive, ArchiveSpec, []}
        ],

    case escript:create(EscriptPath, EscriptSections) of
        ok ->
            ok;
        {error, EscriptError} ->
            error(io_lib:format("could not create escript: ~p", [EscriptError]))
    end,

    %% set executable bits (unix only)
    {ok, #file_info{mode = Mode}} = file:read_file_info(EscriptPath),
    ok = file:change_mode(EscriptPath, Mode bor 8#00111).

-spec prepare_files(escript_artifact_spec()) -> escript_archive_spec().
prepare_files(Artifacts) ->
    Files = expand_to_files_list(Artifacts),
    load_parallel(Files).

-spec expand_to_files_list(escript_artifact_spec()) -> escript_load_spec().
expand_to_files_list(Artifacts) ->
    maps:fold(
        fun(ArchivePath, FSPath, AccOuter) ->
            case filelib:is_dir(FSPath) of
                true ->
                    Files = filelib:wildcard("**", FSPath),
                    lists:foldl(
                        fun(FileShortPath, AccInner) ->
                            FileOrDirPath = filename:join(FSPath, FileShortPath),
                            case filelib:is_dir(FileOrDirPath) of
                                true ->
                                    AccInner;
                                false ->
                                    [
                                        {filename:join(ArchivePath, FileShortPath), FileOrDirPath}
                                        | AccInner
                                    ]
                            end
                        end,
                        AccOuter,
                        Files
                    );
                false ->
                    [{ArchivePath, FSPath} | AccOuter]
            end
        end,
        [],
        Artifacts
    ).

-spec load_parallel(escript_load_spec()) -> escript_archive_spec().
load_parallel([]) ->
    [];
load_parallel(Files) ->
    Self = self(),
    F = fun() -> worker(Self) end,
    Jobs = min(length(Files), erlang:system_info(schedulers)),
    Pids = [spawn_monitor(F) || _I <- lists:seq(1, Jobs)],
    queue(Files, Pids, []).

-spec worker(pid()) -> ok.
worker(QueuePid) ->
    QueuePid ! self(),
    receive
        {load, {ArchivePath, FSPath}} ->
            QueuePid ! {done, FSPath, {ArchivePath, file_contents(FSPath)}},
            worker(QueuePid);
        empty ->
            ok
    end.

-spec file_contents(file:filename()) -> binary().
file_contents(Filename) ->
    case file:read_file(Filename) of
        {ok, Bin} -> Bin;
        Error -> error({read_file, Filename, Error})
    end.

-spec queue(escript_load_spec(), [pid()], escript_archive_spec()) -> escript_archive_spec().
queue([], [], Acc) ->
    Acc;
queue(Files, Pids, Acc) ->
    receive
        Worker when is_pid(Worker), Files =:= [] ->
            Worker ! empty,
            queue(Files, Pids, Acc);
        Worker when is_pid(Worker) ->
            Worker ! {load, hd(Files)},
            queue(tl(Files), Pids, Acc);
        {done, File, Res} ->
            io:format("Loaded ~ts~n", [File]),
            queue(Files, Pids, [Res | Acc]);
        {'DOWN', Mref, _, Pid, normal} ->
            Pids2 = lists:delete({Pid, Mref}, Pids),
            queue(Files, Pids2, Acc);
        {'DOWN', _Mref, _, _Pid, Info} ->
            io:format("ERROR: Compilation failed: ~p", [Info]),
            erlang:halt(1)
    end.
