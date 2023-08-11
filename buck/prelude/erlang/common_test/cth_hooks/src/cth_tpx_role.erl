-module(cth_tpx_role).

% -------- What are cth_tpx roles?? ---------------
%
% # Context
%
% We first need to understand in which order CT calls hook
% callbacks when a suite has more than one hook installed.
%
% Let's assume we have two hooks cth_a and cth_b, and cth_a has
% higher priority than cth_b (so lower numerical value). Then
% CT will call the the hooks in the following order (we show
% suite stuff, group and testcase is the same):
%
% cth_a:pre_init_per_suite
% cth_b:pre_init_per_suite
% cth_a:post_init_per_suite
% cth_b:post_init_per_suite
% ...
% cth_b:pre_end_per_suite
% cth_a:pre_end_per_suite
% cth_b:post_end_per_suite
% cth_a:post_end_per_suite
%
% In particular:
%   - cth_b sees cth_a's results for pre_init_per_suite and post_init_per_suite
%   - cth_a sees cth_b's results for pre_end_per_suite and post_end_per_suite
%
% NB. The order for the post_* functions is arguably wrong, but (historical) reasons
% https://github.com/erlang/otp/issues/7397
%
% # Problem
%
% We want cth_tpx to be able to detect when a group/suite function failed, even
% if the failure comes from a hook. But as we can see, it doesn't matter if we
% give it the highest or lower priority, it will not see the result coming from
% some other hook.
%
% Also we want to compute how long a testcase takes to run, but we will either
% fail to account for the time due to hooks on the init or the end functions.
%
% # Solutions
%
% So we need instead two versions of cth_tpx hook running at the same time, each
% with a different role, that we call "top" and "bot", and with a different
% priorities:
%
% - `top` has the maximum priority (min numerical value), `bot` has the minimum priority
% - `top` handles all the `pre_init_per_*` callbacks, since it runs first there and can
%    compute start_times accurately
% - `top`  handles all `post_end_*` callbacks, since it runs last there and can
%    see all failures from other hooks
% - `bot` is the dual, so handles `pre_end_per_*` and `post_init_per_*` for the same reason
% -  `top` also handles common initializations, since it's `init/2` function is called first

-export_type([
    role/0,
    responsibility/0
]).

-type role() :: top | bot.

-export([
    role_priority/1,
    is_responsible/2
]).

%% @doc Default hook priority for the role
%%
%% - In CT, the hook with the lowest numerical value has "highest priority" and is
%%   initialized first
%% - In Erlang, integers are unbounded. We want top/bot to have "highest" and "lowest"
%%   priority, but there is no such thing, so we do a best effort approach and
%%   pick INT64_MIN and INT64_MAX. If a user really wants to bypass this, of course,
%%   they will.
-spec role_priority(role()) -> integer().
role_priority(top) ->
    INT64_MIN = -9223372036854775808,
    INT64_MIN;
role_priority(bot) ->
    INT64_MAX = 9223372036854775807,
    INT64_MAX.


-type responsibility() ::
    pre_init_per_suite |
    post_init_per_suite |
    pre_init_per_group |
    post_init_per_group |
    pre_init_per_testcase |
    post_init_per_testcase |
    on_tc_fail |
    on_tc_skip |
    pre_init_per_suite |
    post_init_per_suite |
    pre_init_per_group |
    post_init_per_group |
    pre_init_per_testcase |
    post_init_per_testcase |
    terminate.

%% @ doc Partition of responsibilities among both roles
-spec is_responsible(role(), responsibility()) -> boolean().
is_responsible(top, pre_init_per_suite) -> true;
is_responsible(bot, post_init_per_suite) -> true;
is_responsible(top, pre_init_per_group) -> true;
is_responsible(bot, post_init_per_group) -> true;
is_responsible(top, pre_init_per_testcase) -> true;
is_responsible(bot, post_init_per_testcase) -> true;
is_responsible(top, on_tc_fail) -> true;
is_responsible(top, on_tc_skip) -> true;
is_responsible(bot, pre_end_per_suite) -> true;
is_responsible(top, post_end_per_suite) -> true;
is_responsible(bot, pre_end_per_group) -> true;
is_responsible(top, post_end_per_group) -> true;
is_responsible(bot, pre_end_per_testcase) -> true;
is_responsible(top, post_end_per_testcase) -> true;
is_responsible(top, terminate) -> true;
is_responsible(_, _) -> false.
