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
%%%  Build the release resource file, and boot scripts from a given spec file. The spec file format
%%%  is defined in erlang_release.bzl
%%%
%%%  usage:
%%%    boot_script_builder.escript release_spec.term output_dir
%%% @end

-module(boot_script_builder).
-author("loscher@fb.com").

-export([main/1]).

-mode(compile).

-define(EXITSUCCESS, 0).
-define(EXITERROR, 1).

-spec main([string()]) -> ok.
main([ReleaseSpec, OutDir]) ->
    try
        do(ReleaseSpec, OutDir),
        erlang:halt(?EXITSUCCESS)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~s:~s~n", [Type, Reason]),
            erlang:halt(?EXITERROR)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("boot_script_builder.escript release_spec.term output_dir~n").

do(SpecFile, OutDir) ->
    {ok, [
        #{
            "apps" := Apps,
            "lib_dir" := LibDir,
            "name" := RelName,
            "version" := RelVersion
        }
    ]} = file:consult(SpecFile),
    OTPAppMapping = get_otp_apps_mapping(),
    LibDirWildcard = filename:absname(filename:join([LibDir, "*", "ebin"])),

    %% systools requires us to be in the same directory as the .rel file
    %% it also magically discovers the release root and makes all paths
    %% relative to it if the structure is OTP compliant
    ok = file:set_cwd(OutDir),

    build_start_boot(Apps, LibDirWildcard, RelName, RelVersion, OTPAppMapping).

build_start_boot(
    Apps,
    LibDirWildcard,
    RelName,
    RelVersion,
    OTPAppMapping
) ->
    %% OTP apps dependencies are not captures and we need to calculate
    %% them first
    {OTPApps, Others} = lists:partition(
        fun(#{"resolved" := Resolved}) -> Resolved =:= "False" end,
        Apps
    ),
    OTPAppDeps = get_otp_app_deps(OTPApps, OTPAppMapping),

    %% rel file content
    RelFileContent = {
        release,
        {RelName, RelVersion},
        {erts, erlang:system_info(version)},
        [
            case StartType of
                "permanent" -> {erlang:list_to_atom(AppName), AppVersion};
                "load" -> {erlang:list_to_atom(AppName), AppVersion, load}
            end
         || #{
                "name" := AppName,
                "version" := AppVersion,
                "type" := StartType
            } <- Others ++ OTPAppDeps
        ]
    },
    make_boot_script(RelName, RelFileContent, [
        no_warn_sasl,
        no_dot_erlang,
        {script_name, "start"},
        {path, [LibDirWildcard]}
    ]).

make_boot_script(RelName, RelFileContent, Options) ->
    ok = file:write_file(
        io_lib:format("~s.rel", [RelName]),
        io_lib:format("~p.", [RelFileContent])
    ),
    case systools:make_script(RelName, Options) of
        ok ->
            ok;
        Error ->
            error({abort, Error})
    end.

get_otp_apps_mapping() ->
    lists:foldl(
        fun(Path, Acc) ->
            case parse_app_path(Path) of
                skip ->
                    Acc;
                [AppName, AppVersion] ->
                    Acc#{
                        AppName => #{
                            version => AppVersion,
                            app_file => app_file_path(Path, AppName)
                        }
                    }
            end
        end,
        #{},
        code:get_path()
    ).

get_otp_app_deps(OTPApps, OTPAppMapping) ->
    InitialDeps = [
        begin
            #{AppName := #{app_file := AppFile, version := AppVersion}} = OTPAppMapping,
            #{name := AppName, version := AppVersion, deps := Dependencies} = parse_app_file(
                AppFile
            ),
            Dependencies
        end
     || #{"name" := AppName} <- OTPApps
    ],

    InitialAcc = lists:foldl(
        fun(UnVersionedSpec = #{"name" := AppName}, Acc) ->
            % get specific version from toolchain
            #{AppName := #{version := AppVersion}} = OTPAppMapping,
            % replace dynamic version with specific one
            VersionedSpec = UnVersionedSpec#{"version" => AppVersion},
            Acc#{erlang:list_to_atom(AppName) => VersionedSpec}
        end,
        #{},
        OTPApps
    ),

    get_otp_app_deps(lists:flatten(InitialDeps), OTPAppMapping, InitialAcc).

get_otp_app_deps([], _, Acc) ->
    maps:values(Acc);
get_otp_app_deps([App | Rest], OTPAppMapping, Acc) ->
    AppName = erlang:atom_to_list(App),
    #{AppName := #{app_file := AppFile, version := AppVersion}} = OTPAppMapping,
    #{name := AppName, version := AppVersion, deps := Dependencies} = parse_app_file(AppFile),
    Spec = #{
        "name" => AppName,
        "version" => AppVersion,
        "type" => "permanent"
    },
    FilteredDependencies = [Dep || Dep <- Dependencies, not maps:is_key(Dep, Acc)],
    get_otp_app_deps(Rest ++ FilteredDependencies, OTPAppMapping, Acc#{App => Spec}).

app_file_path(Path, Name) ->
    AppFile = unicode:characters_to_list(io_lib:format("~s.app", [Name])),
    filename:join([Path, AppFile]).

parse_app_path(Path) ->
    case string:prefix(Path, [code:lib_dir(), "/"]) of
        nomatch ->
            skip;
        RelPath ->
            case filename:split(RelPath) of
                [AppFolder, "ebin"] ->
                    string:split(AppFolder, "-");
                _ ->
                    skip
            end
    end.

parse_app_file(File) ->
    io:format("~s~n", [File]),
    {ok, [{application, App, Props}]} =
        file:consult(File),
    AppName = erlang:atom_to_list(App),
    Version = proplists:get_value(vsn, Props),
    Dependencies =
        proplists:get_value(applications, Props, []) ++
            proplists:get_value(included_applications, Props, []),
    #{
        name => AppName,
        version => Version,
        deps => Dependencies
    }.
