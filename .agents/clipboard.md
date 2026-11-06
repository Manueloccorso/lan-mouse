# Clipboard Sharing: Analysis and Implementation Plan

## Scope and finalized requirements

Clipboard sharing should support text copied on a Wayland desktop using niri,
with `wl-clipboard` as the initial integration. Synchronization is performed
at client transitions rather than continuously:

- When entering a remote client, the host that is currently sending input
  reads its local clipboard and sends it to the entered client.
- When leaving that client, the host that was receiving input reads its local
  clipboard and sends it back to the host being returned to.
- Only the currently entered client participates. Clipboard data must not be
  broadcast to other configured clients.
- Clipboard sharing is disabled by default and controlled by configuration.
- The initial data type is UTF-8 `text/plain` only.
- Clipboard data is limited to 64 KiB.
- Clipboard sharing is directional at each transition. The sender's setting
  controls whether it sends clipboard data; the receiver's setting controls
  whether it applies received clipboard data.
- If clipboard sharing is disabled on the sender, it sends no clipboard
  events. If it is disabled on the receiver, received clipboard events are
  ignored and the receiver's clipboard is not modified.
- Clipboard failures must never disable or interrupt mouse and keyboard
  sharing.

The sender/receiver terminology is transition-relative. For example, when
the server enters a client, the server sends its clipboard to that client.
When the pointer leaves the client and returns to the server, the client is
the sender and the server is the receiver.

## Existing architecture

The relevant pipeline is:

```text
input-capture -> lan-mouse-ipc / lan-mouse-proto -> input-emulation
```

### Capture side

`src/capture.rs` owns the outgoing transition state. It:

- detects `CaptureEvent::Begin` for a configured client;
- sends `ProtoEvent::Enter`;
- forwards input events while the remote client is active;
- releases the active client on a release bind, remote `Leave`, send failure,
  or capture destruction;
- sends key-up events and modifier reset events before sending `Leave`.

`CaptureTask` has an `active_client` and a `State` distinguishing the enter
handshake (`WaitingForAck`) from normal input forwarding (`Sending`). It also
receives events from the active connection, but currently only treats `Ack`
and `Leave` as relevant there. Clipboard events will need to be accepted in
this path too.

### Emulation side

`src/emulation.rs` receives events from the listener. It currently handles:

- `Enter`: releases local capture, acknowledges the remote peer, and informs
  the service that a remote device entered;
- `Leave`: removes the remote emulation handle and acknowledges the peer;
- `Input`: sends keyboard and pointer events to `input-emulation`;
- `Ping`/`Pong` and `Hello` for liveness and version information.

Clipboard events should be handled in this same listener path, but clipboard
reading/writing should remain separate from `input-emulation`, because it is a
protocol/application transition feature rather than an input emulation
backend feature.

### Protocol and transport

`lan-mouse-proto/src/lib.rs` defines `ProtoEvent` and serializes every event
into a fixed-size byte array. `MAX_EVENT_SIZE` is also used by
`src/connect.rs` as the receive buffer size. Events are transported as
authenticated DTLS/UDP datagrams.

The current protocol supports forward compatibility by skipping undecodable
events in the receive loop. This allows a new peer to communicate normally
with an old peer, although an old peer will not support clipboard sharing.

## Wayland clipboard backend

The initial backend should use the external `wl-clipboard` tools rather than
implementing Wayland clipboard data-control protocols directly:

```sh
wl-paste --no-newline --type text/plain
wl-copy --type text/plain
```

The backend should:

- use asynchronous Tokio subprocesses;
- read text from `wl-paste` stdout;
- write text to `wl-copy` stdin;
- preserve UTF-8 content and trailing-newline behavior as far as the tools
  permit;
- detect missing commands or missing Wayland session variables gracefully;
- treat an empty or unavailable clipboard as a non-fatal condition;
- report command failures without affecting input capture or emulation.

For niri, this requires `wl-clipboard` to be installed and the Lan Mouse
process to inherit the same Wayland session environment, including
`WAYLAND_DISPLAY` and the relevant runtime directory.

The implementation should introduce a fakeable clipboard abstraction so the
network and transition logic can be tested without requiring a Wayland
session. A possible interface is:

```rust
#[async_trait]
trait Clipboard: 'static {
    async fn read_text(&self) -> Result<Option<String>, ClipboardError>;
    async fn write_text(&self, text: &str) -> Result<(), ClipboardError>;
}
```

The first concrete implementation can be a `WaylandClipboard` backed by
`wl-paste` and `wl-copy`. A fake implementation should record writes and
return configured values for reads.

No persistent `wl-paste --watch` process is needed for the finalized
transition-snapshot behavior. Avoiding continuous monitoring also reduces
clipboard feedback-loop risks.

## Configuration semantics

Clipboard sharing should be independently configurable and disabled by
default. A global setting is sufficient initially, for example:

```toml
[clipboard]
enabled = false
```

The setting is intentionally interpreted locally on each host and at each
side of a transition:

```text
sender clipboard setting enabled
    -> sender sends clipboard transfer

receiver clipboard setting enabled
    -> receiver applies a complete received transfer
```

This gives the requested behavior:

| Sender enabled | Receiver enabled | Result |
|---|---|---|
| No | No | No clipboard transfer or application |
| No | Yes | Nothing is sent; receiver remains unchanged |
| Yes | No | Clipboard is sent, but receiver ignores it |
| Yes | Yes | Clipboard is sent and applied |

The setting should apply only to the client involved in the current
transition. It must not cause clipboard data to be sent to every configured
client.

Per-client enablement can be considered later, but is not required for the
initial feature.

## Protocol design

An unrestricted `String` field should not be added directly to `ProtoEvent`.
The existing transport uses fixed-size UDP/DTLS datagrams, and clipboard text
can exceed a safe datagram size even though the total permitted size is only
64 KiB.

The recommended design is a bounded, chunked clipboard transfer. Possible
events are:

```rust
ProtoEvent::ClipboardStart {
    transfer_id: u32,
    total_bytes: u32,
}

ProtoEvent::ClipboardChunk {
    transfer_id: u32,
    chunk_index: u16,
    data: Vec<u8>,
}

ProtoEvent::ClipboardAck {
    transfer_id: u32,
}
```

The exact wire representation can be adjusted to match the existing compact
serialization style. Required properties are:

- payload is UTF-8 bytes;
- total size is limited to 64 KiB;
- each datagram remains below a conservative MTU-safe size;
- incomplete or invalid transfers are discarded;
- transfer state is associated with the peer address and active transition;
- old peers skip the new event types;
- clipboard failures do not affect ordinary input events.

A conservative chunk payload around 1 KiB is reasonable. A transfer should
be rejected before allocation if `total_bytes` exceeds 64 KiB. Reassembly
should also enforce the limit even if the peer sends inconsistent metadata.

### Transfer reliability

DTLS authenticates the datagrams but does not by itself provide the same
application-level transfer semantics as a reliable stream. Clipboard data
should therefore have explicit completion handling:

1. Sender sends `ClipboardStart` and all chunks.
2. Receiver validates and reassembles the transfer.
3. If the receiver-side clipboard setting is enabled, it applies the text.
4. Receiver sends `ClipboardAck` after successful handling.
5. Sender may retry the transfer a small number of times if no acknowledgement
   arrives.

If the receiver is disabled, it can acknowledge receipt without applying the
data, or simply ignore the transfer. The choice should be consistent and
documented; acknowledging receipt without applying is preferable if retries
would otherwise continue unnecessarily.

For enter transitions, the sender should retain the transfer while the enter
handshake is pending so that a failed initial connection attempt does not
silently lose the clipboard.

For leave transitions, `Leave` must still be sent even when the clipboard
read, transfer, or acknowledgement fails.

### Capability negotiation

The existing `Hello` event only carries the peer's commit hash. A future
refinement could add a clipboard capability bit so peers know whether the
other endpoint supports clipboard transfer. This is not strictly required:
new clipboard events can be skipped by old peers, preserving ordinary input
compatibility. Capability negotiation would reduce unnecessary transfers and
make diagnostics clearer.

## Transition integration

### Entering a client

In `src/capture.rs`:

1. On `CaptureEvent::Begin`, determine the entered client.
2. If local clipboard sending is disabled, do not read the clipboard.
3. Otherwise read the current local text clipboard.
4. Reject or skip empty/unavailable/non-UTF-8/oversized data as appropriate.
5. Store a pending transfer associated with the active client and transition.
6. Send `Enter` and the clipboard transfer.
7. Retry while the enter handshake or transfer acknowledgement is pending,
   without delaying pointer or keyboard input indefinitely.

In `src/emulation.rs`:

- receive and reassemble clipboard events for the incoming peer;
- if local clipboard application is disabled, discard the completed transfer;
- otherwise write the complete UTF-8 text with `wl-copy`;
- never let a clipboard write failure terminate emulation.

The remote `Enter` acknowledgement should continue to work independently of
clipboard completion.

### Leaving a client

In `src/capture.rs`, `release_capture` is the transition where the current
receiver becomes the clipboard sender. The desired behavior is:

1. Identify the current active client.
2. If clipboard sending is enabled, read the local clipboard snapshot.
3. Send a bounded clipboard transfer to that client.
4. Continue the existing key-up and modifier-reset cleanup.
5. Send `Leave`.
6. Release local capture.

The exact ordering of clipboard versus key cleanup can be tuned during
implementation, but `Leave` must not be skipped or blocked by clipboard
operations. A timeout or bounded subprocess operation may be needed around
`wl-paste`.

On the original host, the active capture receive path must accept clipboard
events in addition to `Ack` and `Leave`. It should apply completed clipboard
data only if local clipboard application is enabled and only for the active
client/transition.

## Security and safety

Clipboard contents may contain passwords, access tokens, private keys, or
other sensitive information. The feature is therefore disabled by default and
must be clearly documented.

Additional safeguards:

- enforce the 64 KiB limit before allocation;
- enforce the limit during reassembly;
- reject invalid UTF-8;
- discard stale or incomplete transfers after a timeout;
- associate transfer state with the authenticated peer and active client;
- do not apply clipboard data from an unrelated or inactive connection;
- avoid logging clipboard contents;
- log only sizes, transfer IDs, and failure categories;
- ensure clipboard subprocesses cannot block the main input path forever.

## Error behavior

| Situation | Expected behavior |
|---|---|
| `wl-paste` is missing | Rate-limited warning; skip clipboard transfer |
| `wl-copy` is missing | Warning; continue input sharing |
| No clipboard contents | No transfer; continue transition |
| Clipboard is non-text | Skip it; continue transition |
| Clipboard exceeds 64 KiB | Skip and log size; continue transition |
| Invalid UTF-8 | Reject and log; continue transition |
| Transfer is incomplete | Discard after timeout |
| Remote write fails | Log warning; continue input sharing |
| Peer is an old version | Input sharing continues; clipboard is unavailable |
| Clipboard disabled on sender | Send no clipboard events |
| Clipboard disabled on receiver | Ignore received clipboard events |

## Testing plan

### Protocol tests

Add tests covering:

- clipboard event encoding and decoding;
- ASCII and multi-byte UTF-8 text;
- empty text;
- exact 64 KiB payload;
- oversized payload rejection;
- chunk ordering and reassembly;
- duplicate chunks;
- missing chunks;
- invalid UTF-8;
- stale-transfer timeout;
- peer/address isolation;
- unknown clipboard events from new peers being safely ignored by old peers.

### Clipboard backend tests

Use a fake clipboard implementation to avoid requiring Wayland in unit tests.
The fake should:

- return configured text for reads;
- record every attempted write;
- allow simulated read and write failures.

### Transition tests

Verify that:

1. Entering a client reads and sends only the current sender clipboard.
2. The receiver applies the text only when its setting is enabled.
3. Leaving a client reads and sends the current receiver clipboard back.
4. The returning host applies the text only when its setting is enabled.
5. A disabled sender performs no clipboard read and sends no transfer.
6. Clipboard failure does not prevent `Enter`, input, key release, or `Leave`.
7. A transfer for one client cannot be applied to another client.
8. Clipboard contents are never logged.

### Manual niri verification

With `wl-clipboard` installed on two Wayland/niri hosts or sessions:

1. Leave clipboard sharing disabled and verify ordinary input behavior is
   unchanged.
2. Enable it on both hosts.
3. Copy text on the server and enter the client.
4. Paste on the client.
5. Copy different text on the client.
6. Return to the server and paste.
7. Enable sending but disable applying on the receiver, and verify that the
   receiver clipboard is unchanged.
8. Disable sending and verify no clipboard data is transferred.
9. Test multiline text, Unicode, empty clipboard, and text close to 64 KiB.
10. Test missing `wl-clipboard` and verify input sharing remains functional.

Useful commands for manual checks:

```sh
printf 'clipboard test\n' | wl-copy
wl-paste --no-newline --type text/plain
```

## Documentation and packaging

Update `README.md` or `DOC.md` to document:

- the configuration setting and disabled-by-default behavior;
- transition-time rather than continuous synchronization;
- sender and receiver enablement semantics;
- Wayland/niri requirements;
- `wl-clipboard` as a runtime dependency;
- text-only UTF-8 support;
- the 64 KiB limit;
- the security implications of sharing clipboard contents;
- compatibility behavior with older peers.

Nix and distribution packaging documentation should mention `wl-clipboard`
where appropriate.

## Recommended implementation order

1. Add the clipboard configuration setting with a disabled default.
2. Add a fakeable clipboard abstraction and error types.
3. Implement the Wayland backend using `wl-paste` and `wl-copy`.
4. Add protocol clipboard transfer events and serialization tests.
5. Add bounded chunk reassembly, timeout, and acknowledgement handling.
6. Add clipboard handling to incoming emulation.
7. Add sender-side snapshot and transfer behavior on enter.
8. Add sender-side snapshot and transfer behavior on leave.
9. Add active-client and transition validation.
10. Add old-peer compatibility tests and, if useful, capability negotiation.
11. Update documentation and runtime dependency guidance.
12. Run formatting, workspace tests, and all-feature Clippy.

```sh
cargo fmt
cargo test --workspace
cargo clippy --workspace --all-targets --all-features
```

## Likely files to change

- `.agents/clipboard.md`
- `Cargo.toml`
- `src/config.rs`
- `src/capture.rs`
- `src/emulation.rs`
- `src/connect.rs`
- `src/service.rs`, if configuration or event plumbing requires it
- `lan-mouse-proto/src/lib.rs`
- `lan-mouse-proto/Cargo.toml`, if protocol dependencies or versioning change
- `README.md` or `DOC.md`
- a new clipboard module, likely under `src/clipboard.rs`

The initial implementation should not require changes to `input-capture` or
`input-emulation`, unless a later design chooses to make clipboard a reusable
library-level backend.
