# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# A set is useful when the `dedupe` builtin is not applicable. Dedupe looks at
# identity of the value (some kind of pointer) rather than equality, so for
# example doesn't eliminate duplicates of the same string value obtained from
# different places:
#
#     things = ["huh", "huh"]
#     expect(len(dedupe(things)) == 2)
#
#     huh = "huh"
#     things = [huh, huh]
#     expect(len(dedupe(things)) == 1)
#
# In contrast a set compares its entries for equality, not identity, and will
# never contain one entry equal to another entry.
#
# Example usage:
#
#     things = set()
#     for x in somewhere:
#         things.add(x)
#     return things.list()

# Name the record `set_record` to enable users to use `set` to initialize a set.
set_record = record(
    _entries = field(dict),
    list = field("function"),
    # Adds the value to the set, returning whether the value existed in the set
    add = field("function"),
    # Removes the value if the value is in the set, returning whether the value existed in the set
    remove = field("function"),
    # Adds the values to the set, returning the values that were added
    update = field("function"),
    # Returns whether the value is in the set
    contains = field("function"),
    size = field("function"),
)

# For typing a set, you may use `set_type` or `set_record.type`, the former is
# encouraged to avoid leaking the underlying implementation.
set_type = set_record.type

def set(initial_entries: list[""] = []) -> set_type:
    def set_list():
        return self._entries.keys()

    def set_add(v: "") -> bool:
        if self.contains(v):
            return True
        self._entries[v] = None
        return False

    def set_contains(v: "") -> bool:
        return v in self._entries

    def set_remove(v: "") -> bool:
        if self.contains(v):
            self._entries.pop(v)
            return True
        return False

    def set_update(values: list[""]) -> list[""]:
        return filter(None, [v for v in values if not self.add(v)])

    def set_size() -> int:
        return len(self._entries)

    self = set_record(
        _entries = {},
        list = set_list,
        add = set_add,
        remove = set_remove,
        update = set_update,
        contains = set_contains,
        size = set_size,
    )

    self.update(initial_entries)

    return self
