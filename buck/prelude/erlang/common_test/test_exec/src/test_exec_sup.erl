%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% % @format

%% @doc Supervisor that starts two genserver sequentially: the epmd_manager, that will
%% starts the epmd daemon, and the ct_runner, that will launch the test.
%% If one of them stops it entails termination of the whole tree.
-module(test_exec_sup).

-behavior(supervisor).

-export([init/1, start_link/1, start_ct_runner/2]).

-include_lib("common/include/buck_ct_records.hrl").

-spec start_link(#test_env{}) -> {ok, pid()} | 'ignore' | {error, term()}.
start_link(#test_env{} = TestEnv) ->
    supervisor:start_link({local, ?MODULE}, ?MODULE, [TestEnv]).

init([#test_env{} = TestEnv]) ->
    {ok,
        {
            #{
                % strategy doesn't matter as
                % none of the children are to be restarted
                strategy => one_for_one,
                intensity => 0,
                period => 1,
                % If any child terminates, the sup should terminate.
                auto_shutdown => any_significant
            },
            [
                #{
                    id => epmd_manager,
                    start => {epmd_manager, start_link, [TestEnv]},
                    restart => temporary,
                    significant => true,
                    shutdown => 1000,
                    worker => worker
                }
            ]
        }}.

%% @doc Starts the ct_runner as a child of this supervisor.
-spec start_ct_runner(#test_env{}, integer()) -> {ok, pid()} | {error, term()}.
start_ct_runner(#test_env{} = TestEnv, PortEpmd) ->
    % super_method:super_fun(),
    supervisor:start_child(
        ?MODULE,
        #{
            id => ct_runner,
            start => {ct_runner, start_link, [TestEnv, PortEpmd]},
            restart => temporary,
            significant => true,
            shutdown => 1000,
            worker => worker
        }
    ).
