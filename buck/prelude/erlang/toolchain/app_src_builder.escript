%% #!/usr/bin/env escript
%% -*- erlang -*-
%%! +S 1:1 +sbtu +A1

%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc
%%%  Build an .app file from a given list of modules and a template
%%%  .app.src file.
%%%
%%%  usage:
%%%    app_src_builder.escript app_info.term
%%%
%%%  app_info.term format:
%%%
%%%    The file must contain only a single term which is a map
%%%    with the following spec:
%%%
%%%   #{
%%%       "name"                   := <application_name>,
%%%       "output"                 := <path to output .app file>,
%%%       "sources"                := [<path to .erl source file>],
%%%       "applications"           := [<entry to applications field>],
%%%       "included_applications"  := I[<entry to included_applications field>],
%%%       "template"               => <path to an .app.src file>,
%%%       "version"                => <version string>,
%%%       "env"                    => [application env variable],
%%%       "metadata"               => map of metadata
%%%   }
%%%
%%% @end

-module(app_src_builder).

-type application_resource() :: {application, atom(), proplists:proplist()}.
-type mod() :: {atom(), [term()]} | undefined.

-export([main/1]).

-spec main([string()]) -> ok.
main([AppInfoFile]) ->
    try
        do(AppInfoFile)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~s:~s~n", [Type, Reason]),
            erlang:halt(1)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("app_src_builder.escript app_info.term~n").

-spec do(file:filename()) -> ok.
do(AppInfoFile) ->
    #{
        name := Name,
        sources := Srcs,
        output := Output,
        template := Template,
        vsn := Version,
        applications := Applications,
        included_applications := IncludedApplications,
        mod := Mod,
        env := Env,
        metadata := Metadata
    } = do_parse_app_info_file(AppInfoFile),
    VerifiedTerms = check_and_normalize_template(
        Name,
        Version,
        Template,
        Applications,
        IncludedApplications,
        Mod,
        Env,
        Metadata
    ),
    render_app_file(Name, VerifiedTerms, Output, Srcs).

-spec do_parse_app_info_file(file:filename()) ->
    #{
        name := string(),
        vsn := string(),
        sources := [file:filename()],
        output := file:filename()
    }.
do_parse_app_info_file(AppInfoFile) ->
    case file:consult(AppInfoFile) of
        {ok, [
            #{
                "name" := Name,
                "output" := Output,
                "sources" := Sources,
                "applications" := Applications,
                "included_applications" := IncludedApplications
            } = Terms
        ]} ->
            Template = get_template(maps:get("template", Terms, undefined)),
            Mod = get_mod(Name, maps:get("mod", Terms, undefined)),
            Env = get_env(maps:get("env", Terms, undefined)),
            Metadata = get_metadata(maps:get("metadata", Terms, undefined)),
            #{
                name => Name,
                sources => Sources,
                vsn => maps:get("version", Terms, undefined),
                output => Output,
                template => Template,
                applications =>
                    normalize_application([list_to_atom(App) || App <- Applications]),
                included_applications =>
                    [list_to_atom(App) || App <- IncludedApplications],
                mod => Mod,
                env => Env,
                metadata => Metadata
            };
        {ok, Terms} ->
            file_corrupt_error(AppInfoFile, Terms);
        Error ->
            open_file_error(AppInfoFile, Error)
    end.

-spec get_template(file:filename() | undefined) -> application_resource().
get_template(undefined) ->
    {application, '_', []};
get_template(TemplateFile) ->
    case file:consult(TemplateFile) of
        {ok, [Template]} -> Template;
        {ok, Terms} -> file_corrupt_error(TemplateFile, Terms);
        Error -> open_file_error(TemplateFile, Error)
    end.

-spec get_mod(string(), {string(), [string()]} | undefined) -> mod().
get_mod(_, undefined) ->
    undefined;
get_mod(AppName, {ModuleName, StringArgs}) ->
    ModString = unicode:characters_to_list([
        "{", ModuleName, ",[", lists:join(",", StringArgs), "]}."
    ]),
    try
        {ok, Tokens, _EndLine} = erl_scan:string(ModString),
        {ok, Term} = erl_parse:parse_term(Tokens),
        Term
    catch
        _:_ -> module_filed_error(AppName, ModString)
    end.

-spec get_env(map() | undefined) -> [tuple()] | undefined.
get_env(undefined) -> undefined;
get_env(Env) ->
    [{list_to_atom(K), V} || {K, V} <- maps:to_list(Env)].

-spec get_metadata(map() | undefined) -> map().
get_metadata(undefined) -> #{};
get_metadata(Metadata) ->
    maps:from_list([{list_to_atom(K), V} || {K, V} <- maps:to_list(Metadata)]).

-spec check_and_normalize_template(
    string(),
    string() | undefined,
    term(),
    [atom()],
    [atom()],
    mod(),
    [tuple()],
    map()
) ->
    application_resource().
check_and_normalize_template(
    AppName,
    TargetVersion,
    Terms,
    Applications,
    IncludedApplications,
    Mod,
    Env,
    Metadata
) ->
    App = erlang:list_to_atom(AppName),
    Props =
        case Terms of
            {application, App, P} when erlang:is_list(P) ->
                P;
            {application, '_', P} when erlang:is_list(P) ->
                P;
            _ ->
                Msg = io_lib:format(
                    "expect the top-level format of the template to be {application, '~s'/'_', [ ... ]}.~nBut got instead: ~p",
                    [
                        AppName,
                        Terms
                    ]
                ),
                erlang:error(
                    {abort, Msg}
                )
        end,
    VerifiedProps = verify_app_props(
        AppName, TargetVersion, Applications, IncludedApplications, Props
    ),
    Props0 = add_optional_fields(VerifiedProps, [{mod, Mod}, {env, Env}]),
    Props1 = add_metadata(Props0, Metadata),
    {application, App, Props1}.

-spec add_optional_fields(proplists:proplist(), mod() | [tuple()]) -> proplists:proplist().
add_optional_fields(Props, []) ->
    Props;
add_optional_fields(Props, [{_, undefined} | Fields]) ->
    add_optional_fields(Props, Fields);
add_optional_fields(Props, [{K, V0} | Fields]) ->
    V1 = proplists:get_value(K, Props, undefined),
    case V1 of
        undefined ->
            add_optional_fields([{K, V0} | Props], Fields);
        % overwrite the value of empty list in .app.src, for example: {env, []}
        [] ->
            add_optional_fields([{K, V0} | Props], Fields);
        _ ->
            case V0 =:= V1 of
                true -> add_optional_fields(Props, Fields);
                false ->
                    erlang:error(app_props_not_compatible, [{K, V0}, {K, V1}])
            end
    end;
add_optional_fields(Props, [Field | Fields]) ->
    add_optional_fields([Field | Props], Fields).

-spec verify_app_props(string(), string(), [atom()], [atom()], proplists:proplist()) -> ok.
verify_app_props(AppName, Version, Applications, IncludedApplications, Props0) ->
    Props1 = verify_applications(AppName, Props0),
    %% ensure defaults
    ensure_fields(AppName, Version, Applications, IncludedApplications, Props1).

-spec verify_applications(string(), proplists:proplist()) -> ok.
verify_applications(AppName, AppDetail) ->
    case proplists:get_value(applications, AppDetail) of
        AppList when is_list(AppList) ->
            FinalApps = normalize_application(AppList),
            lists:keystore(applications, 1, AppDetail, {applications, FinalApps});
        undefined ->
            AppDetail;
        BadApplicationsValue ->
            applications_type_error(AppName, BadApplicationsValue)
    end.

-spec normalize_application(list(atom())) -> list(atom()).
normalize_application(Applications) ->
    StdLib =
        case lists:member(stdlib, Applications) of
            false ->
                [stdlib];
            true ->
                []
        end,
    Kernel =
        case lists:member(kernel, Applications) of
            false ->
                [kernel];
            true ->
                []
        end,
    Kernel ++ StdLib ++ Applications.

-spec ensure_fields(string(), string(), [atom()], [atom()], proplists:proplist()) ->
    proplists:proplist().
ensure_fields(AppName, Version, Applications, IncludedApplications, Props) ->
    %% default means to add the value if not existing
    %% match meand to overwrite if not existing and check otherwise for
    Defaults = [
        {{registered, []}, default},
        {{vsn, Version}, match},
        {{description, "missing description"}, default},
        {{applications, Applications}, match},
        {{included_applications, IncludedApplications}, match}
    ],
    lists:foldl(
        fun
            ({{Key, _} = Default, default}, Acc) ->
                case lists:keyfind(Key, 1, Acc) of
                    false -> [Default | Acc];
                    _ -> Acc
                end;
            ({{Key, Value} = Default, match}, Acc) ->
                case lists:keyfind(Key, 1, Acc) of
                    false ->
                        [Default | Acc];
                    {Key, Value} ->
                        Acc;
                    %% When 'git' is specified as the version in the .app.src file, it means that
                    %% the version will be calculated dynamically based on the VCS version.
                    %% We consider the version from the Buck target to be authoritative.
                    {vsn, Vsn} when Vsn =:= git orelse Vsn =:= "git" ->
                        [Default | lists:keydelete(vsn, 1, Acc)];
                    Wrong ->
                        value_match_error(AppName, Wrong, Default)
                end
        end,
        Props,
        Defaults
    ).

-spec render_app_file(string(), application_resource(), file:filename(), [file:filename()]) ->
    ok.
render_app_file(AppName, Terms, Output, Srcs) ->
    App = erlang:list_to_atom(AppName),
    Modules = generate_modules(Srcs),
    {application, App, Props0} = Terms,
    %% remove modules key
    Props1 = lists:keydelete(modules, 1, Props0),
    %% construct new terms
    Spec =
        {application, App, [{modules, Modules} | Props1]},
    ToWrite = io_lib:format("~p.\n", [Spec]),
    file:write_file(Output, ToWrite, [raw]).

-spec generate_modules([file:filename()]) -> [atom()].
generate_modules(Sources) ->
    Modules = lists:foldl(
        fun(Source, Acc) ->
            case filename:extension(Source) of
                ".hrl" ->
                    Acc;
                Ext when Ext == ".erl" orelse Ext == ".xrl" orelse Ext == ".yrl" ->
                    ModuleName = filename:basename(Source, Ext),
                    Module = erlang:list_to_atom(ModuleName),
                    [Module | Acc];
                _ ->
                    unknown_extension_error(Source)
            end
        end,
        [],
        Sources
    ),
    lists:usort(Modules).

-spec unknown_extension_error(File :: file:filename()) -> no_return().
unknown_extension_error(File) ->
    Msg = io_lib:format("unsupported extension for source ~s", [File]),
    erlang:error(
        {abort, Msg}
    ).

-spec open_file_error(File :: file:filename(), Error :: term()) -> no_return().
open_file_error(File, Error) ->
    Msg = io_lib:format("cannot open file ~s: ~p", [File, Error]),
    erlang:error(
        {abort, Msg}
    ).

-spec file_corrupt_error(File :: file:filename(), Contents :: term()) -> no_return().
file_corrupt_error(File, Contents) ->
    Msg = io_lib:format("corrupt information in ~s: ~p", [File, Contents]),
    erlang:error(
        {abort, Msg}
    ).

-spec value_match_error(string(), {atom(), term()}, {atom(), term()}) -> no_return().
value_match_error(AppName, Wrong = {_, Value1}, Default = {_, Value2}) when
    is_list(Value1) andalso is_list(Value2)
->
    case io_lib:printable_list(Value1) andalso io_lib:printable_list(Value2) of
        true -> value_match_error_scalar(AppName, Wrong, Default);
        false -> value_match_error_diff(AppName, Wrong, Default)
    end;
value_match_error(AppName, Wrong, Default) ->
    value_match_error_scalar(AppName, Wrong, Default).

value_match_error_diff(AppName, {FieldName, Value1}, {FieldName, Value2}) ->
    Diff = diff_list(Value1, Value2),
    Msg = io_lib:format(
        ("error when building ~s.app for application ~s: the field ~s in "
        "the app.src template does not match with the target definition"),
        [
            AppName, AppName, FieldName
        ]
    ),
    erlang:error(
        {abort, [Msg, "\n", Diff]}
    ).

value_match_error_scalar(AppName, {FieldName, Value1}, {FieldName, Value2}) ->
    Msg = io_lib:format(
        ("error when building ~s.app for application ~s: the field ~s in the "
        "app.src template (~p) does not match with the target definition (~p)"),
        [
            AppName, AppName, FieldName, Value1, Value2
        ]
    ),
    erlang:error(
        {abort, Msg}
    ).

-spec applications_type_error(string(), term()) -> no_return().
applications_type_error(AppName, Applications) ->
    Msg = io_lib:format(
        "error when building ~s.app for application ~s: require a list for applications value but got ~w instead",
        [
            AppName, AppName, Applications
        ]
    ),
    erlang:error(
        {abort, Msg}
    ).

-spec module_filed_error(string(), string()) -> no_return().
module_filed_error(AppName, ModString) ->
    Msg = io_lib:format(
        "error when building ~s.app for application ~s: could not parse value for module field: `~p`",
        [
            AppName, AppName, ModString
        ]
    ),
    erlang:error(
        {abort, Msg}
    ).

diff_list(AppSrcValue, TargetValue) ->
    LCS = lcs(AppSrcValue, TargetValue),
    DiffSpec = construct_diff_spec(LCS, AppSrcValue, TargetValue, []),
    construct_diff(DiffSpec).

construct_diff_spec([], [], [], Acc) ->
    lists:reverse(Acc);
construct_diff_spec([], [RemoveItem | AppSrcValue], TargetValue, Acc) ->
    construct_diff_spec([], AppSrcValue, TargetValue, [{remove, RemoveItem} | Acc]);
construct_diff_spec([], [], [AddItem | TargetValue], Acc) ->
    construct_diff_spec([], [], TargetValue, [{add, AddItem} | Acc]);
construct_diff_spec(
    [CommonItem | LCS], [CommonItem | AppSrcValue], [CommonItem | TargetValue], Acc
) ->
    NewAcc =
        case Acc of
            [{common, N, CommonItems} | Rest] ->
                [{common, N + 1, [CommonItem | CommonItems]} | Rest];
            _ ->
                [{common, 1, [CommonItem]} | Acc]
        end,
    construct_diff_spec(LCS, AppSrcValue, TargetValue, NewAcc);
construct_diff_spec([CommonItem | _] = LCS, [RemoveItem | AppSrcValue], TargetValue, Acc) when
    CommonItem =/= RemoveItem
->
    construct_diff_spec(LCS, AppSrcValue, TargetValue, [{remove, RemoveItem} | Acc]);
construct_diff_spec([CommonItem | _] = LCS, AppSrcValue, [AddItem | TargetValue], Acc) when
    CommonItem =/= AddItem
->
    construct_diff_spec(LCS, AppSrcValue, TargetValue, [{add, AddItem} | Acc]).

-define(LPAD, io_lib:format("~10s", [" "])).
-define(MPAD, io_lib:format("~28s", [" "])).
-define(RPAD, io_lib:format("~45s", [" "])).

construct_diff(Spec) ->
    Header = [
        io_lib:format("           .app.src                           buck2 target~n", []),
        io_lib:format("           ========                           ============~n", [])
    ],
    construct_diff(Spec, [Header]).

construct_diff([], Acc) ->
    lists:reverse(Acc);
construct_diff([{common, N, Items} | Rest], Acc) when N < 5 ->
    Output = [io_lib:format(" ~s~s~n", [?MPAD, format_item(Item)]) || Item <- lists:reverse(Items)],
    construct_diff(Rest, [Output | Acc]);
construct_diff([{common, N, Items} | Rest], Acc) ->
    [Last | _] = Items,
    [First | _] = lists:reverse(Items),
    Output = [
        io_lib:format(" ~s~s~n", [?MPAD, format_item(First)]),
        io_lib:format(" ~s~s~n", [?MPAD, io_lib:format("... ~b more ...", [N - 2])]),
        io_lib:format(" ~s~s~n", [?MPAD, format_item(Last)])
    ],
    construct_diff(Rest, [Output | Acc]);
construct_diff([{remove, Item} | Rest], Acc) ->
    Output = io_lib:format("<~s~s~n", [?LPAD, format_item(Item)]),
    construct_diff(Rest, [Output | Acc]);
construct_diff([{add, Item} | Rest], Acc) ->
    Output = io_lib:format(">~s~s~n", [?RPAD, format_item(Item)]),
    construct_diff(Rest, [Output | Acc]).

format_item(Item) ->
    S = io_lib:format("~w", [Item]),
    case string:length(S) > 30 of
        true ->
            io_lib:format("~.27s...", [S]);
        false ->
            io_lib:format("~.30s", [S])
    end.

%% longest common subsequence from http://rosettacode.org/wiki/Longest_common_subsequence#Erlang
lcs_length([] = S, T, Cache) ->
    {0, maps:put({S, T}, 0, Cache)};
lcs_length(S, [] = T, Cache) ->
    {0, maps:put({S, T}, 0, Cache)};
lcs_length([H | ST] = S, [H | TT] = T, Cache) ->
    {L, C} = lcs_length(ST, TT, Cache),
    {L + 1, maps:put({S, T}, L + 1, C)};
lcs_length([_SH | ST] = S, [_TH | TT] = T, Cache) ->
    case maps:is_key({S, T}, Cache) of
        true ->
            {maps:get({S, T}, Cache), Cache};
        false ->
            {L1, C1} = lcs_length(S, TT, Cache),
            {L2, C2} = lcs_length(ST, T, C1),
            L = lists:max([L1, L2]),
            {L, maps:put({S, T}, L, C2)}
    end.

lcs(S, T) ->
    {_, C} = lcs_length(S, T, #{}),
    lcs(S, T, C, []).

lcs([], _, _, Acc) ->
    lists:reverse(Acc);
lcs(_, [], _, Acc) ->
    lists:reverse(Acc);
lcs([H | ST], [H | TT], Cache, Acc) ->
    lcs(ST, TT, Cache, [H | Acc]);
lcs([_SH | ST] = S, [_TH | TT] = T, Cache, Acc) ->
    case maps:get({S, TT}, Cache) > maps:get({ST, T}, Cache) of
        true ->
            lcs(S, TT, Cache, Acc);
        false ->
            lcs(ST, T, Cache, Acc)
    end.

-spec add_metadata(proplists:proplist(), map()) -> proplists:proplist().
add_metadata(Props, Metadata) ->
    ok = verify_metadata(Props, Metadata),
    Props ++ maps:to_list(Metadata).

-spec verify_metadata(proplists:proplist(), map()) -> ok.
verify_metadata([], _) -> ok;
verify_metadata([{K, V0} | T], Metadata) ->
    case maps:get(K, Metadata, undefined) of
        undefined ->
            verify_metadata(T, Metadata);
        V1 ->
            case V0 =:= V1 of
                true ->
                    verify_metadata(T, Metadata);
                false ->
                    erlang:error(metadata_not_compatible, [{K, V0}, {K, V1}])
            end
    end.
