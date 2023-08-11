%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%% % @format
%%%-------------------------------------------------------------------
%%% @doc behavior module defining callbacks for buck2 test providers
%%%-------------------------------------------------------------------

-module(buck_ct_provider).
-compile(warn_missing_spec).

-include("buck_ct_records.hrl").

-type state() :: term().

-type return_type() :: {ok, state()} | {error, term()}.

-type buck_ct_provider() :: {atom(), state()}.

-type init_argument_type() :: any().

-include_lib("kernel/include/logger.hrl").

-export([
    do_init/2,
    do_pre_listing/1,
    do_post_listing/1,
    do_pre_running/1,
    do_post_running/1,
    do_terminate/1
]).
-optional_callbacks([
    init/2, pre_listing/1, post_listing/1, pre_running/1, post_running/1, terminate/1
]).

% ------------------- Behaviors Callbacks -------------------------

%%% Initialize the state of the provider
-callback init(init_argument_type(), #init_provider_state{}) -> return_type().

%%% Executed before listing and updates the state of the provider
-callback pre_listing(state()) -> return_type().

%%% Executed after listing and updatess the state of the provider
-callback post_listing(state()) -> return_type().

%%% Executed before running the tests and updates the state of the provider
-callback pre_running(state()) -> return_type().

%%% Executed after running the tests and updates the state of the provider
-callback post_running(state()) -> return_type().

%%% Executed as closing.
-callback terminate(state()) -> return_type().

% ------------------- Exported Methods -------------------------

%% @doc Handles calling the init method on providers.
%% Calls the init method if this one is present and updates the state
-spec do_init({atom(), init_argument_type()}, #init_provider_state{}) -> buck_ct_provider().
do_init({ProviderName, Args}, InitState) ->
    execute_method_on_provider(init, ProviderName, InitState, [Args]).

%% @doc Handles calling the pre_listing method on providers.
%% Calls the pre_listing method if this one is present and updates the state.
-spec do_pre_listing(buck_ct_provider()) -> buck_ct_provider().
do_pre_listing({ProviderName, ProviderState}) ->
    execute_method_on_provider(pre_listing, ProviderName, ProviderState, []).

%% @doc Handles calling the post_listing method on providers.
%% Calls the post_listing method if this one is present and updates the state.
-spec do_post_listing(buck_ct_provider()) -> buck_ct_provider().
do_post_listing({ProviderName, ProviderState}) ->
    execute_method_on_provider(post_listing, ProviderName, ProviderState, []).

%% @doc Handles calling the pre_running method on providers.
%% Calls the pre_running method if this one is present and updates the state.
-spec do_pre_running(buck_ct_provider()) -> buck_ct_provider().
do_pre_running({ProviderName, ProviderState}) ->
    execute_method_on_provider(pre_running, ProviderName, ProviderState, []).

%% @doc Handles calling the post_running method on providers.
%% Calls the post_running method if this one is present and updates the state
-spec do_post_running(buck_ct_provider()) -> buck_ct_provider().
do_post_running({ProviderName, ProviderState}) ->
    execute_method_on_provider(post_running, ProviderName, ProviderState, []).

%% @doc Handles calling the terminate method on providers.
%% Calls the terminate method if this one is present and updates the state.
-spec do_terminate(buck_ct_provider()) -> buck_ct_provider().
do_terminate({ProviderName, ProviderState}) ->
    execute_method_on_provider(terminate, ProviderName, ProviderState, []).

% ------------------- Helpers Methods -------------------------

%% @doc Handles the execution of the method on a provider and dealing with the result.
%% Skip if the method is undefined, updates the state or crash according to the return result
%% of the method.
-spec execute_method_on_provider(atom(), atom(), any(), [any()]) -> {atom(), term()}.
execute_method_on_provider(Method, ProviderName, ProviderState, Args) ->
    try safely_execute(ProviderName, Method, Args ++ [ProviderState]) of
        undefined_method ->
            State =
                case Method of
                    init -> #{};
                    _ -> ProviderState
                end,
            {ProviderName, State};
        {ok, NewState} ->
            {ProviderName, NewState};
        {error, Reason} ->
            ErrorMsg = unicode:characters_to_list(
                io_lib:format(
                    "Method ~p on provider ~p with sate ~p ~n returned with error ~p ~n", [
                        Method, ProviderName, ProviderState, Reason
                    ]
                )
            ),
            ?LOG_ERROR(ErrorMsg),
            throw({error_in_provider, {ProviderName, Reason}});
        OtherReturn ->
            case Method of
                terminate ->
                    {ProviderName, ProviderState};
                _ ->
                    ?LOG_DEBUG(
                        "Method ~p on provider ~p with state ~p ~n returned with an unexpeced return ~p ~n",
                        [
                            Method, ProviderName, ProviderState, OtherReturn
                        ]
                    ),
                    {ProviderName, ProviderState}
            end
    catch
        Class:Reason:StackTrace ->
            ErrorMsg = unicode:characters_to_list(
                io_lib:format("Method ~p on provider ~p with sate ~p ~n ~s ~n", [
                    Method,
                    ProviderName,
                    ProviderState,
                    erl_error:format_exception(Class, Reason, StackTrace)
                ])
            ),
            ?LOG_ERROR(ErrorMsg),
            throw({crash_in_provider, {Class, Reason, StackTrace}})
    end.

%% @doc Executes the method if this one is exported, otherwise return undefined_method.
-spec safely_execute(atom(), atom(), [any()]) -> undefined_method | any().
safely_execute(Module, Method, Args) ->
    Module:module_info(),
    case erlang:function_exported(Module, Method, length(Args)) of
        false ->
            undefined_method;
        true ->
            apply(Module, Method, Args)
    end.
