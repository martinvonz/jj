%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format
-module(transform_project_root).

-export([parse_transform/2]).

-type mapping() :: #{file:filename_all() => {true | false, file:filename_all()}}.

-spec parse_transform(Forms, Options) -> Forms when
    Forms :: [erl_parse:abstract_form() | erl_parse:form_info()], Options :: [compile:option()].
parse_transform(Forms, Options) ->
    Mapping = get_mapping(Options),
    rewrite_file_attributes(Forms, Mapping).

-spec rewrite_file_attributes([erl_parse:abstract_form() | erl_parse:form_info()], mapping()) ->
    [erl_parse:abstract_form() | erl_parse:form_info()].
rewrite_file_attributes(Forms, Mapping) ->
    OTPRoot = code:root_dir(),
    [
        case Form of
            {attribute, Anno, file, {File, Line}} ->
                {attribute, Anno, file, {path_relativize(File, OTPRoot, Mapping), Line}};
            _ ->
                Form
        end
     || Form <- Forms
    ].

-spec path_relativize(file:filename(), file:filename(), mapping()) -> file:filename().
path_relativize(
    File,
    OTPRoot,
    Mapping
) ->
    case Mapping of
        #{File := {true, RelativePath}} ->
            RelativePath;
        #{File := {false, MaybeOTPPath}} ->
            case find_in_otp(MaybeOTPPath, OTPRoot) of
                {true, FoundPath} ->
                    FoundPath;
                false ->
                    %% We failed to find the file in OTP, at this point the build should
                    %% have already failed and we let the compiler report the error.
                    MaybeOTPPath
            end;
        _ ->
            %% We don't know where the file comes from, and fall back to the information
            %% in the beam, this happens usually with transitve header includes from OTP.
            File
    end.

-spec find_in_otp(file:filename_all(), file:filename_all()) ->
    {true, file:filename_all()}
    | false.
find_in_otp(Path, OTPRoot) ->
    [App, "include", Header] = filename:split(Path),
    Pattern = filename:join(["lib", [App, "-*"], "include", Header]),
    case filelib:wildcard(Pattern, OTPRoot) of
        [FoundPath] -> {true, filename:join("/otp", FoundPath)};
        _ -> false
    end.

-spec get_source([compile:option()]) -> file:filename_all().
get_source(Options) ->
    case proplists:get_value(compile_info, Options) of
        undefined ->
            error(missing_compile_info);
        CompileInfo ->
            case proplists:get_value(source, CompileInfo) of
                undefined ->
                    error(corrupt_compile_info);
                Source ->
                    Source
            end
    end.

-spec get_mapping([compile:option()]) -> mapping().
get_mapping(Options) ->
    String = os:getenv("BUCK2_FILE_MAPPING", "#{}."),
    {ok, Tokens, _} = erl_scan:string(String),
    {ok, Mapping} = erl_parse:parse_term(Tokens),
    Source = get_source(Options),
    Mapping#{filename:basename(Source) => {true, Source}}.
