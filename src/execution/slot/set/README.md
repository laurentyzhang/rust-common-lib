# Set

`ISet` lets independent transactions update the same contract-owned set in parallel. Even
changes to different elements affect shared state, including the set's size and iteration
order.

Each sequence records its own additions and removals during execution. At generation commit, the library merges the conflict free changes and updates the shared set. The contract can check whether an element is present and iterate over the unique elements in insertion order.

## Typical uses

- **Member registries.** Two users can register different member IDs in parallel. The commit
  adds both IDs when both changes are accepted.
- **Open orders.** Transactions can open and close different orders in the same order book.
- **Token inventories.** Users can deposit different token IDs into the same vault in parallel.

## Contract interface

Each element is a `U256`. A contract uses a 32-byte key to select which set an `ISet` call
operates on. The interface provides `insert`, `remove`, `contains`, `length`, `first`, and
`next`.

Reinserting a removed element puts it at the end, with one exception. If a sequence removes
and then reinserts an element that was already in the set when the sequence began,
reinsertion cancels the removal and the element keeps its original position.

## Compact and indexed formats

All of a set's data lives in its owner's slots, in one of two formats:

- **Compact format**: a linked list, one slot per element. Small, but removing an element needs
  its predecessor, which means walking the list.
- **Indexed format**: an array plus an element → position lookup. Appending, removing and
  membership checks are O(1), at the cost of extra slots.

`Converter::expand` and `Converter::compact` convert between them, one O(n) pass each. Contract
calls work on the indexed format; which layer converts, and when, is up to the caller.

## Slot key

Every slot of a set uses the same 32-byte key; the property byte says what the slot holds.

```text
┌───────────┬───────────┬─────────────┬────────────────────┬─────────────┐
│ prefix(7) │ set ID(6) │ property(1) │ hash or index(16)  │ reserved(2) │
└───────────┴───────────┴─────────────┴────────────────────┴─────────────┘
prefix   = the Arcology namespace prefix, standard for all crdts.
set ID   = keccak(set key)[..6], unique set indentifier.
property = what the slot holds (see each format's table below)
hash     = keccak(element)[..16]   per-element slots
index    = word number             logs; word 0 holds the byte length
reserved = always 0; a key with a non-zero reserved byte isn't a Set key
```

## Compact format: a linked list

`Head` holds the first element and `Tail` the last. Each element `x` except the last has a
`Node` slot, keyed by `x`'s hash, whose value is the element after `x`. The key holds only a
16-byte hash, which can't be turned back into the element, so the value holds the full next
element; the next key is computed from it.

| # | Property | Key field | Value              |
|---|----------|-----------|--------------------|
| 0 | `Length` | zeros     | number of elements |
| 1 | `Head`   | zeros     | first element      |
| 2 | `Tail`   | zeros     | last element       |
| 3 | `Node`   | hash      | element after x    |

Set `{1, 3, 4}`:

```text
 slot              value   meaning
 Length              3     number of elements
 Head                1     first element
 Tail                4     last element
 Node[1]             3     after 1 comes 3
 Node[3]             4     after 3 comes 4

  Head          Tail
   │             │
   ▼             ▼
   1 ──▶ 3 ──▶ 4
```

| Read   | How                                                 | Example |
|--------|-----------------------------------------------------|---------|
| first  | `Head`                                              | 1       |
| last   | `Tail`                                              | 4       |
| walk   | start at `Head`, follow `Node` for `Length` elements | 1, 3, 4 |
| length | `Length`                                            | 3       |

Appending `y` takes two writes: `Node[Tail] = y`, then `Tail = y`; on an empty set also
`Head = y`.

**Element 0.** A zero value reads as an empty slot, so the walk is bounded by `Length`: an empty
`Node[x]` before the last element means the element after `x` is 0. For `{5, 0, 6}`: `Head = 5`,
`Node[5]` is empty, `Node[0] = 6` and `Tail = 6`. An empty set has `Length = 0`, and `Head` and
`Tail` are ignored.

## Indexed format: an array

The elements sit in an array, one per slot, with a reverse lookup from element to position.
Removing an element leaves a hole instead of shifting the array.

| #  | Property   | Key field | Value                                     |
|----|------------|-----------|-------------------------------------------|
| 0  | `Length`   | zeros     | number of live elements                   |
| 9  | `Index`    | index     | elements in order, zero when removed      |
| 10 | `Position` | hash      | position + 1 in `Index`                   |
| 11 | `Holes`    | index     | bitmap of removed positions, 256 per word |
| 12 | `Indexed`  | zeros     | 1 once the set is expanded                |

Set `{1, 3, 4}` right after `expand`:

```text
 slot              value   meaning
 Indexed             1     the set is expanded
 Length              3     number of live elements
 Index #0            1     element at position 0
 Index #1            3     element at position 1
 Index #2            4     element at position 2
 Position[1]         1     1 is at position 0 (stored as position + 1)
 Position[3]         2     3 is at position 1
 Position[4]         3     4 is at position 2
 Holes               0     no positions removed
```

The same set after 3 is removed and 5 is added:

```text
            #0    #1    #2    #3
 Index    ┌─────┬─────┬─────┬─────┐
          │  1  │  0  │  4  │  5  │
          └─────┴─────┴─────┴─────┘
 Holes       0     1     0     0         bit set = removed

 Position[1] = 1   Position[4] = 3   Position[5] = 4   (Position[3] cleared)
 Length = 3
```

| Read      | How                         | Example     |
|-----------|-----------------------------|-------------|
| walk      | index order, skipping holes | 1, 4, 5     |
| next(x)   | continue from `Position[x]` | next(4) = 5 |
| member(x) | `Position[x]` is non-zero   | 4 yes, 3 no |
| length    | `Length`                    | 3           |

A hole is marked in `Holes` because a zero entry alone could also be the element 0.

## Converting between the formats

- **`expand`** walks the list from `Head`, writes each element to `Index` with its `Position`,
  and sets `Indexed`. It does nothing once `Indexed` is set.
- **`compact`** takes each expanded set's live entries in index order and updates the list in
  place: only the `Node` slots whose next element changed are written, then `Head` and `Tail`.
  It then clears `Index`, `Position`, `Holes` and `Indexed`.

```text
 list 1 → 3 → 4 ─expand─▶ Index [1, 3, 4]
                            … remove 3, add 5 …
 list 1 → 4 → 5 ◀─compact─ Index [1, _, 4, 5]
```

## Changes during a block

A transaction's `insert` and `remove` don't touch the indexed format. `Indexer` records them
as pending changes on top of it:

| # | Property          | Key field | Value                                       |
|---|-------------------|-----------|---------------------------------------------|
| 4 | `Added`           | hash      | position + 1 in `DeltaAdded`                |
| 5 | `Removed`         | hash      | 1 if x is pending removal                   |
| 6 | `DeltaAdded`      | index     | added elements, zero when cancelled         |
| 7 | `PendingRemovals` | index     | removed elements, ascending                 |
| 8 | `DeltaCancelled`  | index     | bitmap of cancelled additions, 256 per word |

| Call                                 | Pending change                                                             |
|--------------------------------------|----------------------------------------------------------------------------|
| insert a new element                 | append to `DeltaAdded`; `Added[x]` = its position + 1                      |
| remove a pending addition            | zero its `DeltaAdded` word, set its `DeltaCancelled` bit, clear `Added[x]` |
| remove an element already in the set | add to `PendingRemovals` (ascending); `Removed[x] = 1`                     |
| re-insert a pending removal          | take it out of `PendingRemovals`; clear `Removed[x]`                       |

Reads combine the indexed format with the pending changes:

- **member(x):** pending added, or indexed without a pending removal.
- **walk:** the index (skipping holes and pending removals), then the pending additions.
- **length:** `Length` + entries in `DeltaAdded` − cancelled entries − entries in `PendingRemovals`.

The pending changes accumulate across the jobs of one sequence.

## Merging at generation commit

At each generation commit, `Applier::merge_delta_from` collects every accepted sequence's pending
changes, and `Applier::apply` writes them once into the indexed format. Conflict detection
guarantees that the sequences' changes are independent, so the merge is a plain union:

1. Append the uncancelled additions to `Index` in (block, generation, sequence, job) order, each
   element once, and set their `Position`.
2. For each removal, zero its `Index` entry, set its `Holes` bit, and clear its `Position`
   and `Node`.
3. Update `Length`.

The pending slots themselves are dropped and never reach the block state.

Example, starting from `{1, 3, 4}`:

```text
 sequence A: remove 3, add 5
 sequence B: add 6, add 7, remove 7      (7 is cancelled within B)

            #0    #1    #2    #3    #4
 Index    ┌─────┬─────┬─────┬─────┬─────┐
          │  1  │  0  │  4  │  5  │  6  │      Length = 4
          └─────┴─────┴─────┴─────┴─────┘
 Holes       0     1     0     0     0

 compacted: list 1 → 4 → 5 → 6, Head = 1, Tail = 6
```
