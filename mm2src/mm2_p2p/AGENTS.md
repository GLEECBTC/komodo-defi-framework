# mm2_p2p — Peer-to-Peer Networking

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

libp2p-based networking layer for decentralized communication between KDF nodes.

## Responsibilities

- P2P swarm management via `AtomicDexBehaviour`
- Gossipsub message broadcasting (orderbook, swaps, watchers)
- Request-response direct peer communication
- Peer discovery and exchange
- Relay node mesh maintenance
- Time synchronization validation between peers

## Module Structure

```
src/
├── lib.rs               # Exports, encode/decode_message, NetworkInfo
├── behaviours/
│   ├── atomicdex.rs     # AtomicDexBehaviour, spawn_gossipsub, AdexBehaviourCmd
│   ├── peers_exchange.rs # Peer discovery protocol
│   ├── request_response.rs # Direct peer requests
│   ├── ping.rs          # AdexPing liveness checks
│   └── peer_store.rs    # Peer address storage
├── application/         # Application-layer protocols
│   ├── request_response/
│   │   ├── network_info.rs # NetworkInfoRequest
│   │   └── ordermatch.rs   # Order matching requests
│   └── network_event.rs    # P2P network events
├── p2p_ctx.rs           # P2PContext for MmArc
├── relay_address.rs     # RelayAddress parsing
└── swarm_runtime.rs     # SwarmRuntime async executor
```

## Core Types

### AtomicDexBehaviour

Main `NetworkBehaviour` combining multiple libp2p protocols:

```rust
struct CoreBehaviour {
    gossipsub: Gossipsub,       // Pub-sub messaging
    floodsub: Floodsub,         // Peer address announcements
    peers_exchange: PeersExchange, // Peer discovery
    ping: AdexPing,             // Liveness checks
    request_response: RequestResponseBehaviour, // Direct requests
}
```

### AdexBehaviourCmd

Commands sent to control the P2P swarm:

```rust
enum AdexBehaviourCmd {
    Subscribe { topic },              // Subscribe to gossipsub topic
    Unsubscribe { topic },            // Unsubscribe from topic
    PublishMsg { topic, msg },        // Broadcast message
    PublishMsgFrom { topic, msg, from }, // Broadcast with source
    RequestAnyRelay { req, response_tx }, // Request relays sequentially until success
    RequestPeers { req, peers, response_tx }, // Request specific peers
    RequestRelays { req, response_tx }, // Request all relays, collect responses
    SendResponse { res, response_channel }, // Reply to request
    GetPeersInfo { result_tx },       // Query connected peers
    GetGossipMesh { result_tx },      // Get gossip mesh state
    GetGossipPeerTopics { result_tx }, // Get topics per peer
    GetGossipTopicPeers { result_tx }, // Get peers per topic
    GetRelayMesh { result_tx },       // Get relay mesh
    AddReservedPeer { peer, addresses }, // Add reserved peer
    PropagateMessage { message_id, propagation_source }, // Forward message
}
```

### NodeType

Determines node role in the network:

```rust
enum NodeType {
    Light { network_ports },      // Client node
    Relay { ip, network_ports, wss_certs }, // Server/relay node
    LightInMemory,               // Testing
    RelayInMemory { port },      // Testing
}
```

## Message Topics

P2P messages are organized by topic prefix (defined in mm2_main):

| Prefix | Purpose | Handler Location |
|--------|---------|------------------|
| `orbk` | Order broadcasts | `lp_ordermatch` |
| `swap` | Swap protocol messages (V1) | `lp_swap` |
| `swapv2` | Swap protocol messages (V2) | `lp_swap` |
| `swpwtchr` | Watcher coordination | `swap_watcher` |
| `txhlp` | Transaction helpers | `lp_swap` |
| `PEERS` | Peer address announcements | Floodsub (in mm2_p2p) |

## Initialization Flow

```rust
// 1. Create config
let mut config = GossipsubConfig::new(netid, runtime, node_type, p2p_key);
config.to_dial(seednodes);  // Add seed nodes to dial

// 2. Spawn swarm (returns cmd channel, event receiver, local peer ID)
let (cmd_tx, event_rx, peer_id) = spawn_gossipsub(config, on_poll).await?;

// 3. Store context in MmArc
P2PContext::new(cmd_tx, keypair).store_to_mm_arc(&ctx);
```

## Key Invariants

- **Time sync**: Peers with >20s clock skew are disconnected (`MAX_TIME_GAP_FOR_CONNECTED_PEER = 20`)
- **Mesh maintenance**: Relays maintain `mesh_n_low..mesh_n_high` connections (4-12 for relays, 2-6 for light)
- **Dial cooldown**: Recently dialed peers are skipped for 5 minutes (`DIAL_RETRY_DELAY = 300s`)
- **Message size**: Max ~1MB per message (`MAX_BUFFER_SIZE = 1024 * 1024 - 100`)
- **Default netid**: 6133 (`DEFAULT_NETID`)
- **Announce interval**: Peer address announcements every 600s

## Request-Response Pattern

Direct peer communication for queries:

```rust
// Send request to any relay until success
let (response_tx, response_rx) = oneshot::channel();
cmd_tx.send(AdexBehaviourCmd::RequestAnyRelay {
    req: encoded_request,
    response_tx,
}).await?;
let response = response_rx.await?;
```

Response types:
- `AdexResponse::Ok { response }` — Success with data
- `AdexResponse::None` — No data available
- `AdexResponse::Err { error }` — Error message

## Interactions

| Crate | Usage |
|-------|-------|
| **mm2_main** | `init_p2p()` spawns swarm, event loop processes messages |
| **mm2_core** | `P2PContext` stored in `MmArc` |
| **mm2_net** | `is_global_ipv4` address validation |
| **common** | Executor, SpawnFuture trait |
| **proxy_signature** | (Related) Message signing for proxy auth |

## Transport Configuration

- **Native**: DNS + TCP + WebSocket with Noise encryption, Yamux multiplexing
- **WASM**: WebSocket via browser API
- **Testing**: In-memory transport

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| Peer not connecting | Check seednode addresses, verify netid matches |
| Messages not received | Confirm topic subscription via `GetGossipTopicPeers` |
| Time validation failing | Ensure system clock is synchronized |
| Too many/few connections | Adjust mesh_n parameters in GossipsubConfig |

## Debugging Commands

```rust
// Get connected peers
AdexBehaviourCmd::GetPeersInfo { result_tx }

// Get gossip mesh state
AdexBehaviourCmd::GetGossipMesh { result_tx }

// Get topics per peer
AdexBehaviourCmd::GetGossipPeerTopics { result_tx }

// Get peers per topic
AdexBehaviourCmd::GetGossipTopicPeers { result_tx }

// Get relay mesh
AdexBehaviourCmd::GetRelayMesh { result_tx }
```

## Tests

- Unit: `cargo test -p mm2_p2p --lib`
- Integration: Tests spawn in-memory nodes with `RelayInMemory`
