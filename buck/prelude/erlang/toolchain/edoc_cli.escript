%% #!/usr/bin/env escript
%% -*- erlang -*-
%%! +sbtu

%% This is a fork from the OTP edoc_cli module
%% major changes:
%%   - Errors are propagated instead of being silently ignored
%% =====================================================================
%% Licensed under the Apache License, Version 2.0 (the "License"); you may
%% not use this file except in compliance with the License. You may obtain
%% a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
%%
%% Unless required by applicable law or agreed to in writing, software
%% distributed under the License is distributed on an "AS IS" BASIS,
%% WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
%% See the License for the specific language governing permissions and
%% limitations under the License.

%% @format
%% @doc EDoc command line interface
-module(edoc_cli).
-export([main/1]).

-mode(compile).

main([]) ->
    print(usage());
main(Args) ->
    remove_loggers(),
    Opts = parse_args(Args),
    ok = code:add_pathsa(maps:get(code_paths, Opts)),
    set_output_types(Opts),
    init_warnings_state(),
    try
        case Opts of
            #{run := app, app := App} ->
                edoc:application(App, edoc_opts(Opts));
            #{run := files, files := Files} ->
                edoc:files(Files, edoc_opts(Opts))
        end
    catch
        Class:Reason:StackTrace ->
            case Opts of
                #{errors_as := errors} ->
                    erlang:halt(1);
                #{files := FilesErr, mode := chunks, out_dir := OutputDir} ->
                    [generate_empty_chunk(File, OutputDir) || File <- FilesErr];
                _ ->
                    io:format(
                        standard_error,
                        "unexpected exception when running edoc~n~s~n",
                        [erl_error:format_exception(Class, Reason, StackTrace)]
                    ),
                    erlang:halt(1)
            end
    end,
    case Opts of
        #{warnings_as := errors} ->
            case erlang:get(emitted_warnings) of
                true ->
                    erlang:halt(1);
                false ->
                    ok
            end;
        _ ->
            ok
    end,
    case verify_files_exist(Opts) of
        true -> ok;
        false -> erlang:halt(1)
    end.

remove_loggers() ->
    [logger:remove_handler(H) || H <- logger:get_handler_ids()].

generate_empty_chunk(File, OutputDir) ->
    file:write_file(
        chunk_path(File, OutputDir),
        erlang:term_to_binary(failed_to_build_doc_chunk)
    ).

verify_files_exist(#{files := Files, out_dir := OutputDir}) ->
    lists:all(
        fun(File) ->
            ChunkPath = chunk_path(File, OutputDir),
            case filelib:is_regular(ChunkPath) of
                true ->
                    true;
                false ->
                    io:format(standard_error, "error: coudn't generate ~s~n", [ChunkPath]),
                    false
            end
        end,
        Files
    ).

chunk_path(File, OutputDir) ->
    ModuleName = filename:basename(File, ".erl"),
    filename:join([OutputDir, "chunks", ModuleName ++ ".chunk"]).

set_output_types(#{errors_as := ErrType, warnings_as := WarnType}) ->
    erlang:put(errors_fd, get_fd(ErrType)),
    erlang:put(warnings_fd, get_fd(WarnType)).

get_fd(ignore) -> ignore;
get_fd(_) -> standard_error.

init_warnings_state() ->
    erlang:put(emitted_warnings, false).

parse_args(Args) ->
    Init = #{
        mode => default,
        run => app,
        app => no_app,
        files => [],
        code_paths => [],
        out_dir => undefined,
        include_paths => [],
        continue => false,
        errors_as => errors,
        warnings_as => warnings,
        preprocess => true
    },
    check_opts(maps:without([continue], parse_args(Args, Init))).

parse_args([], #{mode := app, file := Files} = Opts) when length(Files) > 0 ->
    Opts#{run := files};
parse_args([], Opts) ->
    Opts;
parse_args(["-" ++ _ = Arg | Args], #{continue := Cont} = Opts) when Cont /= false ->
    parse_args([Arg | Args], Opts#{continue := false});
parse_args(["-chunks" | Args], Opts) ->
    parse_args(Args, Opts#{mode := chunks});
parse_args(["-o", OutDir | Args], Opts) ->
    parse_args(Args, Opts#{out_dir := OutDir});
parse_args(["-pa", Path | Args], Opts) ->
    #{code_paths := Paths} = Opts,
    parse_args(Args, Opts#{code_paths := Paths ++ [Path]});
parse_args(["-I", Path | Args], Opts) ->
    #{include_paths := Paths} = Opts,
    parse_args(Args, Opts#{include_paths := Paths ++ [Path]});
parse_args(["-app", App | Args], Opts) ->
    parse_args(Args, Opts#{run := app, app := list_to_atom(App)});
parse_args(["-files" | Args], Opts) ->
    parse_args(Args, Opts#{run := files, continue := files});
parse_args([File | Args], #{continue := files} = Opts) ->
    #{files := Files} = Opts,
    parse_args(Args, Opts#{files := Files ++ [File]});
parse_args(["-no-preprocess" | Args], Opts) ->
    parse_args(Args, Opts#{preprocess := false});
parse_args(["-errors_as", OutputType | Args], Opts) ->
    parse_args(Args, Opts#{errors_as := get_output_type(OutputType, Opts)});
parse_args(["-warnings_as", OutputType | Args], Opts) ->
    parse_args(Args, Opts#{warnings_as := get_output_type(OutputType, Opts)});
parse_args([Unknown | _Args], Opts) ->
    print("Unknown option: ~ts\n", [Unknown]),
    quit(bad_options, Opts).

get_output_type(OutputType, Opts) ->
    case lists:member(OutputType, ["errors", "warnings", "ignore"]) of
        true ->
            erlang:list_to_atom(OutputType);
        false ->
            print("Unknown output type: ~ts\n", [OutputType]),
            quit(bad_options, Opts)
    end.

check_opts(Opts) ->
    case Opts of
        #{run := app, app := App} when is_atom(App), App /= no_app -> ok;
        #{run := app, app := no_app} -> quit(no_app, Opts);
        #{run := files, files := [_ | _]} -> ok;
        #{run := files, files := []} -> quit(no_files, Opts)
    end,
    #{
        mode := Mode,
        out_dir := OutDir,
        code_paths := CodePaths,
        include_paths := IncludePaths
    } = Opts,
    lists:member(Mode, [default, chunks]) orelse erlang:error(mode, Opts),
    if
        is_list(OutDir) -> ok;
        OutDir =:= undefined -> ok;
        OutDir =/= undefined -> erlang:error(out_dir, Opts)
    end,
    is_list(CodePaths) orelse erlang:error(code_paths),
    is_list(IncludePaths) orelse erlang:error(include_paths),
    Opts.

quit(Reason, _Opts) ->
    case Reason of
        no_app ->
            print("No app name specified\n");
        no_files ->
            print("No files to process\n");
        bad_options ->
            print("bad command-line options\n")
    end,
    print("\n"),
    print(usage()),
    erlang:halt(1).

edoc_opts(Opts) ->
    EdocOpts =
        case maps:get(mode, Opts) of
            default ->
                [];
            chunks ->
                [
                    {doclet, edoc_doclet_chunks},
                    {layout, edoc_layout_chunks}
                ]
        end,
    App = maps:get(app, Opts),
    OutDir = maps:get(out_dir, Opts),
    [
        {preprocess, maps:get(preprocess, Opts)},
        {includes, maps:get(include_paths, Opts)}
        | EdocOpts
    ] ++
        [{dir, OutDir} || OutDir /= undefined] ++
        [{application, App} || App /= no_app].

print(Text) ->
    print(Text, []).

print(Fmt, Args) ->
    io:format(Fmt, Args).

usage() ->
    "Usage: edoc [options] -app App\n"
    "       edoc [options] -files Source...\n"
    "\n"
    "Run EDoc from the command line:\n"
    "  -app App       \truns edoc:application/2 if no files given; App is the application name\n"
    "  -files Sources \truns edoc:files/2; Sources are .erl files\n"
    "\n"
    "Options:\n"
    "  -chunks        \twhen present, only doc chunks are generated\n"
    "  -o Dir         \tuse Dir for doc output\n"
    "  -I IncPath     \tadd IncPath to EDoc include file search path;\n"
    "                 \tcan be used multiple times\n"
    "  -pa CodePath   \tadd CodePath to Erlang code path; can be used multiple times\n"
    "  -no-preprocess \tturn preprocessing OFF\n"
    "  -errors_as     \tset errors as warnings, errors, or ignore\n"
    "  -warnings_as   \tset errors as warnings, errors, or ignore\n".
