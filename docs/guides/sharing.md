# Sharing between peers

The one-two-three of moving an object between two nodes. This guide runs two
real nodes (your machine and a friend's, or two isolated instances on one
machine) and shares one object end to end. The exact scenario below is what
[the e2e suite](testing.md) automates.

## Setup: two nodes

On machine A (or an isolated instance):

```bash
CANOPEE_APP_ROOT=/tmp/a canopee init
CANOPEE_APP_ROOT=/tmp/a canopee start
```

On machine B (or another terminal):

```bash
CANOPEE_APP_ROOT=/tmp/b canopee init
CANOPEE_APP_ROOT=/tmp/b canopee start
```

> `CANOPEE_APP_ROOT` isolates every path (`identity`, `storage`, socket) so
> two nodes on one machine don't collide — see
> [environment reference](../reference/environment.md). On real machines you
> just `canopee init` + `canopee start` on each.

Wait for the nodes to discover each other (mDNS on a LAN/localhost):

```bash
canopee peers        # (on each node, note the other's peer id)
```

## Step 1: put — private by default

On A:

```bash
echo "hello over the p2p wire" > hello.txt
canopee put hello.txt        # stored, signed — NOT shared
```

Now B tries to fetch the object by its id (grab the hex id with
`canopee put hello.txt --ids` or from `canopee list`):

```bash
# on B:
canopee fetch <A-peer-id> <object-id>
#   → error: fetching an unshared object is refused
```

This is the key behavior: **`put` announces nothing**. B doesn't know the
object exists until A explicitly shares it.

## Step 2: share

On A:

```bash
canopee share hello.txt <object-id>
```

What this did (`share_object` in the runtime — see
[Sharing concept](../concepts/sharing.md)):

1. upserted a `shared: true` `hello.txt` entry in A's home index,
2. published the `(owner, "entry:hello.txt")` pointer so the object is
   resolvable *by name*,
3. announced A as a provider of the object id on the DHT.

Verify A's home index reflects it:

```bash
canopee home
#   hello.txt   <object-id>   Shared: yes
```

## Step 3: discover + fetch

On B:

```bash
canopee find-providers <object-id>
#   → lists A's peer id: it announced itself as a provider
canopee fetch <A-peer-id> <object-id>
canopee list                 # B now holds a verified copy
```

Fetching stores a content-verified copy locally — and from then on *B is a
provider too*, so the object has replicated (see
[Content caching](content-caching.md)).

You can also fetch **by name** instead of by raw id — B resolves A's
`(owner, "entry:hello.txt")` pointer via the DHT first:

```bash
canopee fetch <A-peer-id> hello.txt
```

## Step 4: unshare

On A:

```bash
canopee unshare hello.txt
canopee home
#   hello.txt   <object-id>   Shared: no
```

Now B's fetch is refused again (provider announcement withdrawn, entry
removed from the public pointer set). The object itself stays on both
machines' storages; unsharing only stops *serving* it.

## By any and all three addressing styles

`canopee fetch` and `canopee share` accept:

| What | Example |
|---|---|
| raw object id | 64-hex-char content id |
| a name you stored under | `hello.txt` |
| a peer's shared name | `canopee fetch <peer> hello.txt` |
| a peer identity (`canopee://…`) or username | `canopee fetch canopee://identity/…hello.txt` |

## Recap

1. `put` → private, local, signed.
2. `share <name> <id>` → resolvable by name + discoverable by id.
3. `fetch <peer> <id-or-name>` → verified copy on the fetcher.
4. `unshare <name>` → back to private.

The full semantics live in [Sharing concept](../concepts/sharing.md);
capability-gated access (where fetching is only part of the story) in
[Capabilities in practice](capabilities.md).