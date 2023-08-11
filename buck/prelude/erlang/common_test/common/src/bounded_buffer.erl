%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%%%-------------------------------------------------------------------
%%% @doc
%%% A bounded FIFO queue
%%% @end
%%% % @format

-module(bounded_buffer).

-compile(warn_missing_spec).

%% Public API
-export([new/1, put/2, get_elements/1]).
-export_type([buffer/1]).

-opaque buffer(T) :: {{queue:queue(T), integer()}, integer(), boolean()}.

-spec new(integer()) -> buffer(term()).
new(MaxElements) -> {{queue:new(), 0}, MaxElements, false}.

-spec put(buffer(T), T) -> buffer(T).
put({{Queue, LenQueue}, MaxElements, Truncated}, Chars) when LenQueue < MaxElements ->
    {{queue:in(Chars, Queue), LenQueue + 1}, MaxElements, Truncated};
put({{Queue, LenQueue}, MaxElements, _Truncated} = _Buffer, Chars) when LenQueue == MaxElements ->
    {_, NewQueue} = queue:out(Queue),
    {{queue:in(Chars, NewQueue), LenQueue}, MaxElements, true}.

-spec get_elements(buffer(T)) -> {[T], boolean()}.
get_elements({{Queue, _LenQueue}, _MaxElements, Truncated}) ->
    {queue:to_list(Queue), Truncated}.
