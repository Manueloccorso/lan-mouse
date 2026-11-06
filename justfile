# Lan Mouse development and deployment helpers.
#
# The deployment recipes assume that the binary is installed at
# /usr/local/bin/lan-mouse and that the service is managed as a user service.

binary := "/usr/local/bin/lan-mouse"
service := "lan-mouse.service"
service_file := "service/lan-mouse.service"
desktop_file := "it.moccorso.LanMouse.desktop"
desktop_dir := "/usr/local/share/applications"
user_service_dir := env("HOME") / ".config/systemd/user"
release_binary := "target/release/lan-mouse"

default:
    just --list --unsorted

# Start the user service.
service-start:
    systemctl --user start {{ service }}

# Enable the user service for the graphical session and start it now.
service-enable:
    systemctl --user enable --now {{ service }}

# Stop the user service.
service-stop:
    systemctl --user stop {{ service }}

# Restart the user service.
service-restart:
    systemctl --user restart {{ service }}

# Disable the user service and stop it now.
service-disable:
    systemctl --user disable --now {{ service }}

# Build a release binary with every feature enabled.
build-complete:
    cargo build --release --all-features

[doc("Build a release binary for a lightweight Wayland/niri setup.
This includes layer-shell capture and uinput emulation, but omits GTK, X11, libei, and the remote-desktop portal backends.")]
build-reduced:
    cargo build --release --no-default-features --features layer_shell_capture,uinput_emulation

[doc(" Install the existing release binary and user service, then restart Lan Mouse.
Run `just build-complete` or `just build-reduced` first.
")]
deploy:
    #!/usr/bin/env bash
    echo "Deployng your custom lan-mouse build to your system..."
    set -euo pipefail
    echo "Testing that the release binary exists and is executable..."
    test -x "{{ release_binary }}"
    echo "Installing {{ release_binary }} to {{ binary }}..."
    sudo install -Dm755 "{{ release_binary }}" "{{ binary }}"
    echo "Installing {{ desktop_file }} to {{ desktop_dir }}/{{ desktop_file }}..."
    sudo install -Dm644 "{{ desktop_file }}" "{{ desktop_dir }}/{{ desktop_file }}"
    sudo update-desktop-database "{{ desktop_dir }}" 2>/dev/null || true
    echo "Installing {{ service_file }} to {{ user_service_dir }}/{{ service }}..."
    install -Dm644 "{{ service_file }}" "{{ user_service_dir }}/{{ service }}"
    echo "Enabling and starting {{ service }}..."
    systemctl --user daemon-reload
    systemctl --user enable --now "{{ service }}"
    systemctl --user restart "{{ service }}"
