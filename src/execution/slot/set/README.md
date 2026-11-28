# Set

`ISet` lets independent transactions update the same contract-owned set in parallel. Even
changes to different elements affect shared state, including the set's size and iteration
order.

Each sequence records its own additions and removals during execution. Generation commits only merge the conflict free changes; the persistent store applies them to the shared set once, at block end. The contract can check whether an element is present and iterate over the unique elements in insertion order.

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

`Converter::expand` converts the list into the index as the cache loads a Set, and the
persistent store's committers apply the pending changes in each format, one O(n) pass each.
Contract calls work on the indexed format; only MDBX keeps the list.

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
Every commit rebuilds it without holes; the cache holds nothing else.

| #  | Property   | Key field | Value                                     |
|----|------------|-----------|-------------------------------------------|
| 0  | `Length`   | zeros     | number of live elements                   |
| 9  | `Index`    | index     | elements in order                         |
| 10 | `Position` | hash      | position + 1 in `Index`                   |
| 11 | `Holes`    | index     | unused: commits leave no holes            |
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
```

The same set once a commit removes 3 and adds 5:

```text
            #0    #1    #2
 Index    ┌─────┬─────┬─────┐
          │  1  │  4  │  5  │
          └─────┴─────┴─────┘

 Position[1] = 1   Position[4] = 2   Position[5] = 3   (Position[3] cleared)
 Length = 3
```

| Read      | How                         | Example     |
|-----------|-----------------------------|-------------|
| walk      | index order                 | 1, 4, 5     |
| next(x)   | continue from `Position[x]` | next(4) = 5 |
| member(x) | `Position[x]` is non-zero   | 4 yes, 3 no |
| length    | `Length`                    | 3           |

`Length` bounds the index, so an entry of zero is the element 0.

## Converting between the formats

- **`expand`** walks the list from `Head`, writes each element to `Index` with its `Position`,
  and sets `Indexed`. It does nothing once `Indexed` is set.
- **`compact`** applies the pending changes to the committed list and updates it in place: only
  the `Node` slots whose next element changed are written, then `Head` and `Tail`. The indexed
  format never reaches MDBX.
- **`reindex`** applies them to the committed index, rewriting it without holes.

```text
 list 1 → 3 → 4 ─expand─▶ Index [1, 3, 4]
                            … remove 3, add 5 …
 list 1 → 4 → 5 ◀─compact─ committed [1, 3, 4] + pending ─reindex─▶ Index [1, 4, 5]
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
- **walk:** the index (skipping pending removals), then the pending additions.
- **length:** `Length` + entries in `DeltaAdded` − cancelled entries − entries in `PendingRemovals`.

The pending changes accumulate across the jobs, sequences and generations of one block.

## Merging until block end

Every store above the persistent store only merges: at each sequence and generation commit,
`Applier::merge_delta_from` reads each accepted state's own changes over its base from the flags
it changed, and `Applier::apply` writes them into the shared pending changes. The committed index
stays as the block found it. Conflict detection guarantees that the states' changes are
independent, so the merge is a plain union:

1. The base's additions stay, less the cancelled ones; the new uncancelled additions follow in
   (block, generation, sequence, job) order, each element once.
2. The base's removals stay, less the restored ones; the new removals join, ascending.

So a later generation that re-inserts a committed element pending removal restores it at its
original position.

## Applying at block end

The persistent store applies the merged pending changes once, through its two committers, which
get the same accepted batches. Each takes a Set's committed elements, drops the removals, and
appends the additions in batch order, each element once:

- **The cache** (`Converter::reindex`) rewrites `Index` and `Position` from the first position
  that changed, with no holes, and keeps no pending changes. A Set it doesn't hold is skipped and
  loads whole from MDBX on its next read.
- **MDBX** (`Converter::compact`) expands its list internally and writes back only the `Node`
  slots whose successor changed, a removed element's emptied, then `Head`, `Tail` and `Length`.

Example, starting from `{1, 3, 4}`:

```text
 sequence A: remove 3, add 5
 sequence B: add 6, add 7, remove 7      (7 is cancelled within B)

 pending:  added [5, 6]   removed [3]

            #0    #1    #2    #3
 Index    ┌─────┬─────┬─────┬─────┐
          │  1  │  4  │  5  │  6  │      Length = 4
          └─────┴─────┴─────┴─────┘

 list 1 → 4 → 5 → 6, Head = 1, Tail = 6
```
