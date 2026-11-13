# Niri directional Lan Mouse focus

## Objective

Add an opt-in Niri integration with the smallest practical impact on the main
Lan Mouse architecture.

When running under Niri, the user can bind:

```text
lan-mouse cli niri focus_left
lan-mouse cli niri focus_right
```

The command means “perform Niri's directional monitor-focus operation”. It must:

1. Focus the local monitor in that direction when one exists.
2. Otherwise, if an active Lan Mouse client exists at that edge, transfer input
   to it through the ordinary Lan Mouse edge-capture path.
3. Otherwise, do nothing and return a useful result.

The initial scope is horizontal focus only (`left` and `right`). Vertical
directions and window/workspace movement are follow-up work.

## Explicit non-goals

- Do not move windows or workspaces between computers.
- Do not add a new Lan Mouse wire-protocol message.
- Do not make the client/server relationship symmetric in static configuration.
- Do not create a virtual Niri output or virtual monitor.
- Do not intercept arbitrary Niri keybindings transparently.
- Do not alter normal physical mouse-edge behavior.

## Configuration

Add a top-level opt-in section to the existing TOML configuration:

```toml
[seamless_niri]
enabled = true
```

Requirements:

- The field is absent/false by default.
- The setting is local to the current Lan Mouse instance.
- The command refuses to operate unless the current session has a usable
  `$NIRI_SOCKET`.
- The `left` and `right` fields explicitly enable transfer attempts for those
  edges. They do not assert that a peer is currently connected.
- Keep the change isolated to the existing `ConfigToml` deserialization and a
  `Config::niri_enabled()` getter.

## Why the transfer should reuse edge capture

The normal Lan Mouse path is already responsible for the difficult parts:

```text
pointer enters edge capture surface
  → CaptureEvent::Begin
  → existing connection sends ProtoEvent::Enter
  → peer acknowledges
  → relative mouse/keyboard events are forwarded
```

The Niri command should not duplicate handshake, active-client selection,
pressed-key tracking, release handling, or protocol behavior. It should request
the existing capture task to perform the same edge transition for the active
client at `left` or `right`.

This is an intra-process request to Lan Mouse's own capture task, not a new
message between Lan Mouse instances.

## Pointer-movement feasibility

The requested implementation would ideally emulate moving the local pointer to
the selected edge, allowing the existing layer-shell edge surface to generate a
real pointer-enter event. This must be investigated before implementation:

### Preferred path: compositor-supported pointer movement

Check the target Niri version for a supported protocol or IPC action that can
move the pointer to a logical coordinate at the edge. Candidate mechanisms
include:

- `wp_pointer_warp_v1`;
- a Niri-specific pointer-warp IPC action; or
- a supported virtual-pointer protocol.

The implementation must verify support at runtime and fail safely if unavailable.
An ordinary Wayland client cannot assume it may warp the compositor pointer.
`wlr-virtual-pointer` is compositor-specific and must not be assumed to work on
Niri without testing.

If pointer movement is supported, the CLI should:

1. Query the focused output and its logical geometry from Niri.
2. Select a coordinate just inside the requested local output edge.
3. Ask the compositor to move the pointer there.
4. Let the existing Lan Mouse edge surface generate the normal capture begin.
5. Return without directly sending an `Enter` protocol event.

This gives the closest possible behavior to a user physically moving the mouse
to the edge.

### Fallback path: synthetic edge transition

If Niri cannot move the pointer, factor the existing capture-begin handling so
the capture task can receive a local “begin at position” request. This is not a
new network protocol message and should reuse the same code path after the
backend-specific capture activation step.

The fallback must be explicit about its limitation: it can request Lan Mouse
input capture, but it cannot claim to have physically moved the compositor's
visible pointer. It must not create full-screen overlay surfaces or leave a
pointer lock active unless the selected backend confirms that the pointer is
actually captured. Those approaches caused stale focus, invisible pointers,
and wrong-monitor return behavior in earlier experiments.

If the backend cannot activate pointer capture without a real pointer-enter
serial, focus-only keyboard transfer may be possible, but the command must
report that mouse transfer is unavailable rather than silently leaving the
mouse local.

## Niri IPC design

Implement a small Niri IPC client in the CLI crate or another narrow internal
module. It should communicate directly with `$NIRI_SOCKET` using newline-delimited
JSON and ignore unknown response fields.

Required requests:

- `FocusedOutput`;
- `Outputs`; and
- native directional monitor focus (`FocusMonitorLeft` or `FocusMonitorRight`).

The decision algorithm is:

1. Read the focused output.
2. Read all logical output rectangles.
3. Find a local candidate in the requested direction. A candidate may have a
   gap; it must be directionally beyond the current rectangle and overlap it on
   the perpendicular axis. Corner-only contact is not sufficient.
4. If a candidate exists, issue Niri's native focus-monitor action.
5. If no candidate exists, ask Lan Mouse to transition to the active client at
   that edge.

The helper must not run a native action and infer success from later state
events. Niri's public IPC does not expose failed action attempts reliably.

The public command names should be exactly:

```text
lan-mouse cli niri focus_left
lan-mouse cli niri focus_right
```

The Niri configuration must replace the corresponding native bindings:

```kdl
binds {
    Mod+Ctrl+H { spawn "lan-mouse" "cli" "niri" "focus_left"; }
    Mod+Ctrl+L { spawn "lan-mouse" "cli" "niri" "focus_right"; }
}
```

## Minimal Lan Mouse integration

The CLI checks only the configured edge flag. It does not query configured
clients or incoming peers before injecting motion; an absent peer simply means
that the normal edge capture has nothing to activate.

No new frontend request, capture request, or network protocol message is
required. The command injects a temporary Linux uinput mouse movement and lets
the existing edge capture path decide whether a peer is present.

### Peer direction metadata

The `ProtoEvent::Enter(Position)` direction already exists in the main branch.
It is not necessary to change the wire protocol for this feature.

When a peer enters this device, existing receive-side state can know:

- the peer address;
- the direction from which it entered; and
- its connection/fingerprint information.

For this narrower feature, use that metadata only if the active edge-client
lookup already exposes it. Do not introduce reverse-transfer protocol changes
or dynamic incoming-peer promotion in the first implementation. That work had a
large impact surface and produced stale-pointer/focus interactions in the
previous attempt.

## Implementation phases

### Phase 0: establish the pointer capability

Before changing Lan Mouse capture state:

1. Identify the exact Niri version and enabled Wayland protocols.
2. Test whether Niri provides pointer warp or virtual-pointer support.
3. Verify whether a client can move the pointer onto the existing Lan Mouse
   edge surface and receive a normal `wl_pointer::Enter` event.
4. Test this with the current `layer-shell` backend and with the backend
   selected automatically.
5. If no supported mechanism exists, stop and document that exact pointer
   emulation is unavailable on that Niri version; do not implement a misleading
   synthetic mouse transfer.

### Phase 1: configuration and Niri command

1. Add the optional `[seamless_niri]` section and its edge fields.
2. Add `focus_left` and `focus_right` CLI commands.
3. Validate `$NIRI_SOCKET` and return typed errors.
4. Implement output geometry parsing and unit tests.
5. Implement native local monitor focus.

At this phase, remote fallback can be a dry-run log/result so local Niri
behavior is independently verifiable.

### Phase 2: pointer-to-edge integration

1. Add a narrow backend capability for moving the pointer to an edge, only if
   Phase 0 confirms a supported mechanism.
2. Pass the focused output identity/geometry only as local request data; do not
   add it to the Lan Mouse network protocol.
3. Move the pointer to the edge and verify that the existing edge capture
   surface produces the normal `CaptureEvent::Begin`.
4. Use the existing capture path to transfer to the active client.
5. Add timeout/error handling without leaving a pointer lock or keyboard focus
   behind.

### Phase 3: documentation and manual validation

Document:

- the opt-in configuration;
- the two exact CLI commands;
- the required Niri binding replacement;
- the requirement that the target Lan Mouse client is active;
- the Niri/version limitation for pointer movement; and
- the fact that no window/workspace movement is performed.

Manual tests:

1. One local monitor and one active client on each horizontal edge.
2. Two local monitors with and without gaps between them.
3. Rightmost-to-center local focus followed by center-to-remote transfer.
4. Remote transfer and return using normal mouse movement.
5. Repeated shortcuts, disabled capture, disconnected clients, and held
   modifiers.
6. Non-Niri session with `[seamless_niri].enabled = true`.

## Acceptance criteria

The feature is complete only if:

- a local monitor always wins over a remote fallback;
- a gap between local monitors does not cause a remote transfer;
- the remote fallback uses the same normal edge-capture path as physical mouse
  movement;
- no full-screen synthetic capture surface or stale pointer lock remains after
  transfer;
- keyboard and mouse behavior are consistent;
- no wire-protocol changes are required; and
- existing non-Niri capture behavior remains unchanged.
