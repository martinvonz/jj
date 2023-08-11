%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% Artefact directory file management.
%%% Used by TPX to upload diagnostic reports.
%%% @end
%%% % @format

-module(test_artifact_directory).
-compile(warn_missing_spec).

-include_lib("kernel/include/logger.hrl").

%% Public API
-export([prepare/1, link_to_artifact_dir/2]).

-export_type([dir_path/0]).

-type dir_path() :: file:filename() | undefined.

% Gets the artifactory directory path.
% This one might be undefined if tpx is ran in offline mode.
-spec artifact_dir() -> dir_path().
artifact_dir() ->
    ArtifactDir = os:getenv("TEST_RESULT_ARTIFACTS_DIR"),
    case ArtifactDir of
        false ->
            undefined;
        Dir ->
            filelib:ensure_path(Dir),
            Dir
    end.

-spec with_artifact_dir(fun((file:filename()) -> X)) -> X | ok.
with_artifact_dir(Func) ->
    case artifact_dir() of
        undefined -> ok;
        Dir -> Func(Dir)
    end.

artifact_annotation_dir() ->
    ArtifactAnnotationDir = os:getenv("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR"),
    case ArtifactAnnotationDir of
        false ->
            undefined;
        Dir ->
            filelib:ensure_path(Dir),
            Dir
    end.

-spec with_artifact_annotation_dir(fun((file:filename()) -> X)) -> X | ok.
with_artifact_annotation_dir(Func) ->
    case artifact_annotation_dir() of
        undefined -> ok;
        Dir -> Func(Dir)
    end.

% Collect, create and link the logs and other relevant files in
% the artefacts directory.
-spec prepare(file:filename()) -> ok.
prepare(ExecutionDir) ->
    with_artifact_dir(
        fun(_ArtifactDir) ->
            link_tar_ball(ExecutionDir),
            case find_log_private(ExecutionDir) of
                {error, log_private_not_found} ->
                    ok;
                LogPrivate ->
                    [
                        link_to_artifact_dir(File, LogPrivate)
                     || File <- filelib:wildcard(filename:join(LogPrivate, "**/*.log")),
                        filelib:is_regular(File)
                    ]
            end,
            ok
        end
    ).

-spec link_to_artifact_dir(file:filename(), file:filename()) -> ok.
link_to_artifact_dir(File, Root) ->
    with_artifact_dir(
        fun(ArtifactDir) ->
            RelativePath =
                case string:prefix(File, Root) of
                    nomatch ->
                        ?LOG_ERROR("~s should be a prefix of ~s", [Root, File]),
                        error(unexpected_path);
                    Suffix ->
                        String =
                            case unicode:characters_to_list(Suffix) of
                                Result when is_list(Result) -> Result
                            end,
                        string:strip(String, left, $/)
                end,
            FullFileName = lists:flatten(string:replace(RelativePath, "/", ".", all)),
            case filelib:is_file(File) of
                true ->
                    file:make_symlink(File, filename:join(ArtifactDir, FullFileName)),
                    Annotation = artifact_annotations:create_artifact_annotation(FullFileName),
                    dump_annotation(Annotation, FullFileName);
                _ ->
                    ok
            end
        end
    ).

-spec link_tar_ball(file:filename()) -> ok.
link_tar_ball(LogDir) ->
    with_artifact_dir(
        fun(ArtifactDir) ->
            {_Pid, MonitorRef} = spawn_monitor(fun() ->
                erl_tar:create(
                    filename:join(ArtifactDir, "execution_dir.tar.gz"), [{"./", LogDir}], [
                        compressed
                    ]
                )
            end),
            receive
                {'DOWN', MonitorRef, _Type, _Object, _Info} -> ok
            after 15000 -> ok
            end
        end
    ).

dump_annotation(Annotation, FileName) ->
    with_artifact_annotation_dir(
        fun(ArtifactAnnotationDir) ->
            AnnotationName = FileName ++ ".annotation",
            {ok, AnnotationFile} = file:open(
                filename:join(ArtifactAnnotationDir, AnnotationName), [write]
            ),
            file:write(AnnotationFile, artifact_annotations:serialize(Annotation))
        end
    ).

find_log_private(LogDir) ->
    Candidates = [
        Folder
     || Folder <- filelib:wildcard(filename:join(LogDir, "**/log_private")), filelib:is_dir(Folder)
    ],
    case Candidates of
        [] -> {error, log_private_not_found};
        [LogPrivate | _Tail] -> LogPrivate
    end.
