%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% This is a fork from the OTP edoc_report module
%% major changes:
%%   - add error codes for edoc errors
%%   - print errors to stderr and warnings to stdio
%%   - limit output to a single line
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
%%
%% Alternatively, you may use this file under the terms of the GNU Lesser
%% General Public License (the "LGPL") as published by the Free Software
%% Foundation; either version 2.1, or (at your option) any later version.
%% If you wish to allow use of your version of this file only under the
%% terms of the LGPL, you should delete the provisions above and replace
%% them with the notice and other provisions required by the LGPL; see
%% <http://www.gnu.org/licenses/>. If you do not delete the provisions
%% above, a recipient may use your version of this file under the terms of
%% either the Apache License or the LGPL.
%%
%% @private
%% @copyright 2001-2003 Richard Carlsson
%% @author Richard Carlsson <carlsson.richard@gmail.com>
%% @see edoc
%% @end
%% =====================================================================

%% @doc EDoc verbosity/error reporting.

-module(edoc_report).

%% Avoid warning for local functions error/{1,2,3} clashing with autoimported BIF.
-compile({no_auto_import, [error/1, error/2, error/3]}).
-export([
    error/1,
    error/2,
    error/3,
    report/2,
    report/3,
    report/4,
    warning/1,
    warning/2,
    warning/3,
    warning/4
]).

error(What) ->
    error([], What).

error(Where, What) ->
    error(0, Where, What).

error(Line, Where, S) when is_list(S) ->
    report(Line, Where, S, []);
error(Line, Where, {S, D}) when is_list(S) ->
    report(Line, Where, S, D);
error(Line, Where, {format_error, M, D}) ->
    report(Line, Where, M:format_error(D), []).

warning(S) ->
    warning(S, []).

warning(S, Vs) ->
    warning([], S, Vs).

warning(Where, S, Vs) ->
    warning(0, Where, S, Vs).

warning(L, Where, S, Vs) ->
    erlang:put(emitted_warnings, true),
    report(erlang:get(warnings_fd), "warning: ", L, Where, S, Vs).

report(S, Vs) ->
    report([], S, Vs).

report(Where, S, Vs) ->
    report(0, Where, S, Vs).

report(L, Where, S, Vs) ->
    report(erlang:get(errors_fd), "", L, Where, S, Vs).

report(FD, Prefix, L, Where, S, Vs) ->
    WhereStr = where(Where),
    LineStr =
        if
            is_integer(L), L > 0 ->
                io_lib:format("at line ~w: ", [L]);
            true ->
                ""
        end,
    Code = get_code(S),
    MessageStr = io_lib:format(patch_format_string(S), Vs),
    Total = io_lib:format("[~s] ~s ~s ~s ~s~n", [Code, Prefix, WhereStr, LineStr, MessageStr]),
    print_report(FD, unicode:characters_to_list(Total)).

print_report(ignore, _) -> ok;
print_report(FD, Report) -> io:put_chars(FD, Report).

where({File, module}) ->
    io_lib:format("~ts, in module header: ", [File]);
where({File, footer}) ->
    io_lib:format("~ts, in module footer: ", [File]);
where({File, header}) ->
    io_lib:format("~ts, in header file: ", [File]);
where({File, {F, A}}) ->
    io_lib:format("~ts, function ~ts/~w: ", [File, F, A]);
where([]) ->
    io_lib:format("~s: ", [edoc]);
where(File) when is_list(File) ->
    File ++ ": ".

get_code("XML parse error: ~p.") ->
    "EDOC001";
get_code("error in XML parser: ~P.") ->
    "EDOC002";
get_code("nocatch in XML parser: ~P.") ->
    "EDOC003";
get_code("heading end marker mismatch: ~s...~s") ->
    "EDOC004";
get_code("`-quote ended unexpectedly at line ~w") ->
    "EDOC005";
get_code("``-quote ended unexpectedly at line ~w") ->
    "EDOC006";
get_code("```-quote ended unexpectedly at line ~w") ->
    "EDOC007";
get_code("reference '[~ts:...' ended unexpectedly") ->
    "EDOC008";
get_code("cannot handle guard") ->
    "EDOC009";
get_code("error reading file '~ts': ~w") ->
    "EDOC010";
get_code("file not found: ~ts") ->
    "EDOC011";
get_code("expected file name as a string") ->
    "EDOC012";
get_code("@spec arity does not match.") ->
    "EDOC013";
get_code("@spec name does not match.") ->
    "EDOC014";
get_code("must specify name or e-mail.") ->
    "EDOC015";
get_code("redefining built-in type '~w'.") ->
    "EDOC016";
get_code("multiple '<...>' sections.") ->
    "EDOC017";
get_code("multiple '[...]' sections.") ->
    "EDOC018";
get_code("missing '~c'.") ->
    "EDOC019";
get_code("unexpected end of expression.") ->
    "EDOC020";
get_code("multiple @~s tag.") ->
    "EDOC021";
get_code("tag @~s not allowed here.") ->
    "EDOC022";
get_code("bad macro definition: ~P.") ->
    "EDOC023";
get_code("cannot find application directory for '~s'.") ->
    "EDOC024";
get_code("recursive macro expansion of {@~s}.") ->
    "EDOC025";
get_code("undefined macro {@~s}.") ->
    "EDOC026";
get_code("unexpected end of macro.") ->
    "EDOC027";
get_code("missing macro name.") ->
    "EDOC028";
get_code("bad macro name: '@~s...'.") ->
    "EDOC029";
get_code("reference to untyped record ~w") ->
    "EDOC030";
get_code("'~s' is not allowed - skipping tag, extracting content") ->
    "EDOC031";
get_code(
    "cannot handle spec with constraints - arity mismatch.\n"
    "This is a bug in EDoc spec formatter - please report it at "
    "https://bugs.erlang.org/\n"
    "Identified arguments: ~p\n"
    "Original spec: ~s\n"
) ->
    "EDOC032";
get_code(
    "cannot annotate spec: "
    "function and spec clause numbers do not match\n"
) ->
    "EDOC033";
get_code(
    "EDoc @spec tags are deprecated. "
    "Please use -spec attributes instead."
) ->
    "EDOC034";
get_code(
    "EDoc @type tags are deprecated. "
    "Please use -type attributes instead."
) ->
    "EDOC035";
get_code("redefining built-in type '~w'.") ->
    "EDOC036";
get_code("duplicated type ~w~s") ->
    "EDOC037";
get_code("missing type ~w~s") ->
    "EDOC038";
get_code("tag @~s not recognized.") ->
    "EDOC039";
get_code(_NoCategory) ->
    "EDOC000".

patch_format_string(S) ->
    Replacements = [
        {"~p", "~0p"},
        {"~P", "~0P"},
        {"\n", " "}
    ],
    unicode:characters_to_list(
        lists:foldl(
            fun({From, To}, String) ->
                string:replace(String, From, To)
            end,
            S,
            Replacements
        )
    ).
