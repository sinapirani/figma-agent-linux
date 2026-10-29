# Figma Agent for Linux

Lets figma.com in your browser use the fonts installed on your Linux machine,
like the official Figma Agent does on macOS and Windows. Unofficial; not
affiliated with Figma.

A single static binary (~1 MB): runs on any Linux distro — Ubuntu, Debian,
Fedora, RHEL/CentOS, Arch/Manjaro, openSUSE, Alpine, NixOS… — with nothing else
installed. Builds for x86_64 (Intel/AMD) and aarch64 (ARM).

## Install

Download the archive for your CPU (`x86_64` for Intel/AMD, `aarch64` for ARM)
from [Releases](https://github.com/sinapirani/figma-agent-linux/releases/latest), then:

    tar xzf figma-agent-linux-*.tar.gz
    cd figma-agent-linux-*/
    ./install.sh

No root needed. It installs to `~/.local/share/figma-agent-linux` and starts at
login (systemd user service, or an XDG autostart entry on non-systemd distros).

Then in Figma: **Preferences → Permissions and helpers → Connect**, and allow
the browser prompt.

## Run without installing

    ./figma-agent

## Fonts

Fonts known to fontconfig plus `/usr/share/fonts`, `/usr/local/share/fonts`,
`~/.local/share/fonts` and `~/.fonts`: TrueType, OpenType, collections and
WOFF (also when saved with a `.ttf` name). WOFF2 is not supported.
New fonts are picked up automatically the next time Figma loads the list.

Add more folders with `FIGMA_AGENT_FONT_DIRS=/path/one:/path/two` (edit the
`Environment=` line in `~/.config/systemd/user/figma-agent.service`, then
`systemctl --user daemon-reload && systemctl --user restart figma-agent`).

A font doesn't show up? See what the agent reads from it:

    figma-agent --inspect /path/to/font.ttf

## Uninstall

    ~/.local/share/figma-agent-linux/uninstall.sh

## Details

Listens on `http://127.0.0.1:44950` and only answers browser requests from
`*.figma.com`. It serves only font files it indexed itself.
Logs: `journalctl --user -u figma-agent -f`.

## Building

Needs Rust with the musl targets:

    rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
    ./build.sh            # both architectures -> dist/*.tar.gz
