# Calimero storage

## Background and purpose

Within the Calimero Network we want to be able to share data between nodes as a
basic premise. Fundamentally this involves the implementation of checks to
ensure that data is legitimate, along with supportive data structures to aid in
the synchronisation of the data and merging of any changes, plus appropriate
mechanisms to share and propagate the data over the network. Beyond this, the
desired feature set shapes the approaches we take to satisfy the requirements.

Notably, the following features are wanted:

- Intervention-free merging, with automatic conflict resolution
- Full propagation of data across the Calimero Network of nodes
- Ability to denote some nodes as having special attributes
- Eventual consistency of general-purpose data
- Ability to enforce validation channels of important data
- Local storage of unshared personal data
- Partial sharing, based on preference or permissions

The overall concept can be described simply as being, "the ability to store any
kind of data for any purpose" (which is a big mission statement!).

## Technical design

There are a number of facets to the technical design, which are spread across
networking, storage, synchronisation/merging, validation, and privacy. Notably,
both validation and privacy rely on encryption, and so share certain aspects.

This document discusses the storage aspect.

### Features

From a storage perspective, it is important that whatever structure or solution
is chosen, it should support scaling without inherent constraint, while enabling
all of the features identified as being desirable. To this end, we can start off
by identifying a few basic principles:

- Keeping primary data separate from files and larger objects
- Treating data items as atomic and indivisible
- Supporting metadata of various kinds (see below)
- Allowing partial representation of the metadata as well as of the full data

It is worth exploring some of these areas and defining the concepts involved.

### Concepts

In this paradigm, we can define "metadata" as trivial data or properties about
the primary data, with the files and larger objects being represented as blobs
or objects of arbitrary size. By way of example:

- A photo of a kitten would consist of the image file being the stored object,
  and at the very least a reference to the object being in the primary date, and
  likely other details such as the filename, size, modification date, and
  anything else that might be considered important to record but which is of a
  relatively small size.

- A calendar event might be entirely primary data, or might also store the ICS
  file alongside.

- System metadata such as may be added for purposes of transmission is not part
  of the payload but attached to it to give extra information such as routing
  information.

We can consider that the full data object and the primary data should be
encrypted, and the message payload should be signed, and system metadata (i.e.
system properties) may or may not be signed according to their type.

### Data types

Here, therefore, we should focus on the storable data, which excludes the system
metadata.

For the "full" data, i.e. the blobs or objects, this is quite easy: each is
considered immutable and indivisible, and linked to a primary data entry. They
can be stored as files on disk, or as entries in a large object store, or
whatever makes the most sense – storing them is simply a standard filesystem
problem (with the note that they should be stored in encrypted form).

Note that although the objects are considered to be atomic, this does not mean
that they have to be updated blindly – we can still allow for efficient updates
of stored files such as might be done using tools such as rsync, whereupon only
the updated pieces of the file would be transmitted.

For the primary data, which is the primary unit of transmission, anything going
into a data entity is considered atomic, immutable, and indivisible. From this
perspective, then, we do not have to be concerned with intra-item merging, as
only a whole item can be updated at once.

### Further details

The following properties are of significance:

- Each metadata block is of minimal, yet somewhat arbitrary, size.
- The metadata blocks belong to a hierarchy, composed of leaves and branches.
- The hierarchy should support partial replication.

The model used for storage is Merkle-CRDT, which a) supports atomic replication
and synchronisation, and b) organises into a structure whereby each level
validates what belongs to it. In this way the hierarchy can be validated on a
per-message basis but also on a structural basis, which is important to ensure
ongoing consistency (for situations such as error, bitrot, or tampering).

## Technical implementation

### CRDT mechanism

The fundamental approach is to use CRDTs, with Merkle-CRDT. There are two main
approaches, which are operation-based and state-based. Although they do
essentially the same thing, the practical implementations differ, and there are
pros and cons.

State-based CRDTs (referred to specifically as CvRDTs, i.e. convergent
replicated data types) are somewhat simpler to implement, and rely on gossip
protocol, and at first glance may appear to be appealing. However, they are
absolutely not suitable for our purposes, as they rely on the entire state being
transmitted. The states are then merged. This is off-putting for the Calimero
Network, as it would be inefficient, plus it would prevent us from having the
ability to implement key features such as partial replication.

Operation-based CRDTs (referred to specifically as CmRDTs, i.e. commutative
replicated data types) work by transmitting only the update operation. This is
more efficient, and more suitable for integration with required Calimero
features. However, a drawback is that the approach usually requires the network
layer to ensure complete transfer of all data updates, with any being missed or
duplicated. Still, as the order does not matter, our planned approach mitigates
for this aspect via (a) employing a last-write-wins strategy, and (b) using
Merkle trees to check validity, with catch-up/sync being performed upon error.

In this way, we achieve an optimum mechanism for the Calimero Network which
supports all of the required features and is both efficient and reliable, whilst
working with our establishing networking design.

### CRDT structures

A number of CRDT types are mathematically defined, including:

- **GCounter**: A grow-only counter that can be incremented on any node.
- **PNCounter**: A counter that can be incremented and decremented.
- **GSet**: A grow-only set that can have elements added to it.
- **TwoPSet**: A two-phase set that can have elements added and removed.
- **LWWElementSet**: A last-write-wins set of elements using sets and unions.
- **ORSet**: A set that can have elements added and removed, with support for
  multiple replicas.
- **Sequence/List/OrderedSet**: Uses an ordered set as an alternative to
  operational transformation.

These fundamental types are not currently exposed to application developers.

Our technical approach is primarily based upon LWWElementSets, but with some
enhancements that add in some of the features of ORSets and OrderedSets.
Specifically, LWWElementSets use timestamps, and a combination of add and remove
sets, whereas ORSets use "tags". In our approach we "tag" each element with a
unique ID, and using this to carry out element changes, and manage structure via
the sets, which aligns with LWWElementSet logic. In this way we end up with an
optimised and flexible approach, which allows the functionality we need, and the
ability to grow and further specialise the types in use over time.

### Developer interface

#### Low-level access

There are some fundamental operations exposed, which at a basic level are to
"get" and "set" for an identified element, or to update the element membership
for a set. In terms of structure, then, we can say:

- Elements are individually-identifiable, and have data and properties
- Collections are sets of elements
- Nodes in the tree can be pure collections, or also have elemental properties

In order to access an element or collection, there are two approaches: by unique
ID, or by path. The unique ID (essentially a "tag" in CRDT terms) allows
individual identity to be tracked throughout location changes, and also allows
for retained and consistent access. Meanwhile, path access allows for querying
the tree at different levels, to retrieve either a single element or a set of
elements. Notably, that set may change between queries, which is why the unique
IDs are important for our use case.

An example of a tree might be:

```
Auctions
    Auction
        Bids
            Bid
            Bid
            Bid
    Auction
    Auction
```

In this way, we can easily see that querying the tree by path is trivial and
immediately understandable, and matches similar interfaces such as RESTful APIs
and GraphQL. We could arrive at a set of bids via "Auctions/2/Bids", or a
specific bid via "Auctions/2/Bids/3".

Meanwhile, with each element also having a unique ID, we can interrogate the
storage at any point in order to interact directly with that element regardless
of where it may be moved to. (In this example, it is highly-unlikely that
elements would move to different places in the tree, but in other cases such
movement would be possible.)

We can identify this form of access as being fairly "low-level", in that it
allows direct access to the elements in storage. Note that it does not expose
the CRDT mechanisms, as those are invisible and handled internally. So from the
developer's perspective, they are simply interacting with a hierarchical
structure similar to a document store in a NoSQL database. Any updates are
applied without intervention, and any CRDT types employed are not directly
translatable to what the developer sees.

Bear in mind that, as mentioned earlier on in this document, we store both the
"primary data" (which is the application's data, but not the "full" data of e.g.
files on disk), and also the "metadata" which is the additional information that
the Calimero system adds, outside of the application's data, in order to manage
the storage. The developer only has direct access to their stored data/metadata,
but we expose certain of our own "system metadata" via functions, e.g. to obtain
a last update timestamp.

#### Higher-level access

We provide a managed high-level interface for developers to use, which abstracts
away the underlying get/set operations, and sidesteps the access by path and ID.
We do this in a way that maps code-defined structures onto the storage, and
handles the details in the background.

For instance, in order to model our tree, we need only two basic concepts:
collections and elements. We also need to support a node being both. We do not
particularly care about what types are used on the Rust side; only that we need
to know how they fit together. Therefore, we can imagine something along these
lines:

```rust
struct Auction {
    owner_id: Id,
    bids: Vec<Bid>,
}
struct Bid {
    auction_id: Id,
    price: Decimal,
    time: DateTime<Utc>,
}
```

Now, clearly we don't know anything about these types, or how to handle them. We
also do not want to get involved in the complexities of supporting specific
types for the most part, as our pattern does not include the ability for us to
"look inside" an atomic element. To us, the elements are black-boxes.

Therefore, we present an approach by which the developer can tell us how to
model their structure, and how it relates together. Considering our
fundamentals, we have collections and elements, where elements are atomic units.
So annotating the structs in this manner works very nicely:

```rust
#[derive(AtomicUnit)]
struct Auction {
    owner_id: Id,

    #[Collection]
    bids: Vec<Bid>,
}

#[derive(AtomicUnit)]
struct Bid {
    auction_id: Id,
    price: Decimal,
    time: DateTime<Utc>,
}
```

Note also that if a collection does not have additional properties, i.e. if it
is essentially a tuple struct with a single field, we can annotate the entire
struct instead of the field:

```rust
#[derive(Collection)]
struct Auction {
    bids: Vec<Bid>,
}
```

Or:

```rust
#[derive(Collection)]
struct Auction(Vec<Bid>);
```

It is also worth noting that there is no limit to how many collections a struct
could contain, and that the hierarchy and path structure supports this.

Note: We do not validate relationships at a storage level; it is up to the
application to keep those consistent.

Note also that a particular collection or struct can be identified as the "root"
– this is not shown here.

Finally, note also that only those items annotated as being collections or
atomic units are represented in the tree: all other fields will be serialised
into the "black box" of the atomic unit that owns them.

### Metadata

Properties such as id, timestamp, and hash are part of the system metadata that
we track for each part of the structure. The timestamp is used for
reconciliation, and the hash is the Merkle hash used for validation of
descendants. Nodes may or may not have data – elements always will, but nodes
only will if they are elements as well as collections.

We also store additional system metadata such as permissions, ownership,
privacy, replication strategy, and more.

### Private data

As well as shared data, we also need to support private data. Private data is
identical to shared data in terms of access and use – it's just private. Where
the private data gets stored is up to the application developer – it may be in
an entirely separate part of the tree, e.g. under a special root, or it may be
interspersed with the other data in the tree – or both. This doesn't matter to
us; there is essentially no difference between private and shared data from a
storage perspective, and it is all state. To clarify: there is no "private
state" and "shared state"; they are both just part of the application state.

The best way to achieve this would be by employing annotations in similar
fashion to those used to denote collections and elements. We absolutely want to
stay away from the type system, as that's not directly related to storage and
will introduce unnecessary factors. Additionally, it's somewhat orthogonal as
introducing a type implies there is a "thing", and this is not a "thing " but a
property of a thing. We don't want to impose anything on the application
developer, and want to stay out of their way. Therefore, something like this
would work nicely:

```rust
#[derive(AtomicUnit)]
struct Auction {
    owner_id: Id,

    #[Private]
    is_starred: bool,

    #[Collection]
    bids: Vec<Bid>,
}
```

In this manner we can annotate any structure or part of a structure as being
private.

Now, the example above has been carefully-chosen on purpose. It is easy to
understand the outcome of annotating a collection or element as private. But
what happens when a single field is marked as such? In this situation, the
effect is simply that the private field is excluded from serialisation when
synced.
