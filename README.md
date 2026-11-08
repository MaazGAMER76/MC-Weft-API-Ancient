<p align="center"><img src="docs/img/banner.png" alt="Weft"></p>

<p align="center">
  <a href="https://discord.gg/zTEekbrc7U"><img src="https://img.shields.io/badge/Discord-join%20the%20server-5865F2?logo=discord&logoColor=white" alt="Discord"></a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.95%2B-B7410E?logo=rust&logoColor=white" alt="Rust 1.95+">
  <img src="https://img.shields.io/badge/graphics-Vulkan%20%7C%20GL%202.1%20%7C%20D3D10-A41E22" alt="Vulkan, OpenGL 2.1, D3D10">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-2A6CB0" alt="Windows, Linux and macOS">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-3B2F7E" alt="MPL-2.0"></a>
</p>

<p align="center">
  <a href="https://github.com/MaazGAMER76/MC-Weft-API-Ancient/releases/latest"><b>Download</b></a> ·
  <a href="#playing-on-older-gpus"><b>Older GPUs</b></a> ·
  <a href="#controls"><b>Controls</b></a> ·
  <a href="docs/modding.md"><b>Modding</b></a> ·
  <a href="#project-status"><b>Project status</b></a> ·
  <a href="#contributing"><b>Contributing</b></a>
</p>

Weft is a voxel sandbox engine written in Rust. The game itself is a mod, `mods/base`, built only on the public SDK: it has no private hooks into the engine, so anything it does, another mod can do or change. Weft is not a Minecraft clone and does not read Minecraft worlds; it plays like one and borrows its numbers where that makes sense.

> Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft. Weft contains no Minecraft code or assets.

## Features

- The world is made of cubic 16x16x16 chunks, so there is no limit on height or depth, and the terrain is generated on the GPU, which allows distant terrain to be drawn far past the view distance. World generation is deterministic and includes caves, several dimensions, and biomes whose colors blend into each other on the borders.

- Instead of random ticks, the world is split into regions that are ticked in parallel, and plants, furnaces and other timed blocks catch up on the time nobody was there for, so crops keep growing and furnaces keep smelting even while their chunks are not loaded.

- Blocks can emit colored RGB light and use connected or randomized texture variants, textures can be animated or glowing, and models are made in Blockbench with keyframe animations. The look of the game can be changed with resource packs and shader packs.

- As for gameplay, there are survival and creative modes, crafting and smelting, mobs with pathfinding, liquids that can be picked up with a bucket regardless of which liquid it is, machines powered by energy, and moving structures that you can stand and build on.

- Multiplayer works peer to peer: Open to Friends in the pause menu gives you a ticket, and a friend can join with it without any server or port forwarding.

- Mods are native libraries behind a C ABI, packed as zip files and loaded in parallel. Content is registered through builders, behavior hooks into the game through events, interceptors and value chains, and recipes, loot and world generation are driven by tags, which is described in more detail in [Modding](#modding).

- For development, textures, translations, shaders and UI can be reloaded in game, and there are debug overlays, a recipe viewer, and crash reports that name the mod which was running at the moment of the crash.

## Download

Get the Windows build from [**Releases**](https://github.com/MaazGAMER76/MC-Weft-API-Ancient/releases/latest), unpack the zip and run `weft.exe`. This fork ships the **legacy-capable build**: it does not need Vulkan and runs on GPUs as old as Intel HD Graphics (Ironlake, 2010) through OpenGL 2.1 or Direct3D 10, with a CPU fallback. It runs on modern GPUs too.

The zip is a complete, portable game folder:

| Inside the zip | What it is |
|---|---|
| `weft.exe` | The game (release build, no installer, no runtime downloads) |
| `assets/core/` | The engine's own textures, shaders and fonts |
| `run/mods/base.zip` | The game content itself, packed as a mod |
| `run/config/render.toml` | Which renderer starts first (see below) |
| `checks/` | Small scripted smoke tests (`--script checks/gl21-terrain.txt`) |

Because a `run/` folder sits next to the executable, everything the game writes — saves, settings, logs, screenshots, player mods — stays inside that folder. Copy the folder anywhere, including a USB stick.

Reference machine this build targets: Intel Core i3 M 380, Intel HD Graphics (Ironlake, OpenGL 2.1 / D3D10 feature level 10_0), 4 GB RAM, Windows 10. If your GPU has Vulkan and you want the standard build instead, the upstream project publishes one at [Weft-Rust/Weft](https://github.com/Weft-Rust/Weft/releases).

Weft ships with its own textures. If Minecraft is installed through the official launcher or Prism, Weft builds a resource pack from that copy when it starts. If you have a custom installation path, you can set it with `MINECRAFT_DIR_LOOKUP`. The pack stays on your computer and can be turned off in Settings > Resource Packs.

## Playing on older GPUs

The engine's modern renderer uses bindless textures, storage buffers and indirect draws, which OpenGL 2.1 and D3D10 hardware cannot do. This fork adds two legacy backends that draw the same world a different way: the engine's face records are decoded on the CPU into plain vertex arrays, every block texture is packed into a single atlas, and the shaders are plain GLSL 120 / HLSL 4.0. Lighting, biome tint blending, fog and animated textures work the same as on the modern path, at the textures' original resolution.

- **OpenGL 2.1** (`gl21`) — the default in the shipped zip; works on Ironlake's Windows 10 driver.
- **Direct3D 10** (`d3d10`) — the same world path through feature level 10_0, for machines where the GL driver is the weak side.
- **CPU** (`cpu`) — last-resort software fallback so the game always starts.

If the chosen renderer cannot start, the game tries the others in order (D3D10, GL 2.1, CPU) instead of giving up. Pick one explicitly with `--render gl21`, `--render d3d10` or `--render cpu`, or change `renderer = "gl21"` in `run/config/render.toml`. `--render-select` shows a chooser at startup.

Useful flags for older machines: `--view 4` (shorter view distance), `--size 1366x768` (window size), `--no-vsync`. To boot straight into a world without menus: `weft.exe --world myworld --seed 1`.

Status of the legacy path: the world renders — block faces and free quads (terrain, trees, plants, flowers) with lighting, biome tint blending, fog and animated textures. Entities, items, the UI, particles, hand and sky passes are still being ported, so on a legacy renderer the game is currently best started with `--world` as above. The modern Vulkan path (source build) renders everything.

## Controls

| Key | Action | | Key | Action |
|---|---|---|---|---|
| W A S D | Move | | E | Inventory |
| Mouse | Look | | Q | Drop item |
| Left click | Attack / break | | T | Chat |
| Right click | Use / place | | / | Command |
| Middle click | Pick block | | R / U | Recipes / uses |
| Space | Jump | | 1–9 | Hotbar |
| Shift | Sneak | | Tab | Player list |
| Ctrl | Sprint | | F1 | Hide HUD |
| F | Toggle fly | | F2 | Screenshot |
| Esc | Pause / back | | F3 | Debug overlay |
| F5 | Perspective | | F8 | Reload assets |
| F11 | Fullscreen | | | |

Every action is rebindable in the settings; mods can register their own keys on the same table.

## Build from source

Requires Rust 1.95 or newer.

```sh
git clone https://github.com/MaazGAMER76/MC-Weft-API-Ancient.git
cd MC-Weft-API-Ancient
cargo run --release
```

That builds the default renderer set (Vulkan first). To build exactly what the legacy zip ships — no Vulkan, smaller compile:

```sh
cargo build --release -p weft -p base -p testkit --no-default-features --features weft/renderer-gl21,weft/renderer-d3d10,weft/renderer-cpu
```

Run it from the repository root. Release builds pack each mod in `mods/` into `run/mods/` on start; settings, saves and logs go to `run/`. `cargo run --release -- --dist <dir>` makes a standalone build to share. `cargo test --release --workspace` runs the test suite; `target/release/weft --test all` runs the in-game test kit.

A game from a release writes nothing next to itself, so it can live in a read-only folder and be replaced by an update:

| | Saves, mods, packs, logs | Settings | Cache |
|---|---|---|---|
| Linux | `$XDG_DATA_HOME/weft` (`~/.local/share/weft`) | `$XDG_CONFIG_HOME/weft` (`~/.config/weft`) | `$XDG_CACHE_HOME/weft` (`~/.cache/weft`) |
| Windows | `%APPDATA%\Weft` | `%APPDATA%\Weft\config` | `%LOCALAPPDATA%\Weft` |
| macOS | `~/Library/Application Support/Weft` | `.../Weft/config` | `~/Library/Caches/Weft` |

A game that already has a `run/config` folder next to it keeps everything in `run/`, as older versions did; make that folder to keep a copy portable (the release zip already has it). `--run-dir <dir>` puts everything in one folder of your choice.


## Modding

Copy [`examples/examplemod`](examples/examplemod): every kind of registration is there as a commented example. Then read the [modding cheat sheet](docs/modding.md); the guides for worldgen, machines, models, UI, tooltips, textures, CTM, resource packs and shaders are in [`docs/`](docs). [`mods/base`](mods/base) is the game itself and [`mods/testmod`](mods/testmod) shows a mod extending and replacing it without engine changes.

## Project status

Early. The mod interface and the save format can still change between versions. A mod built against another engine version refuses to load and asks to be rebuilt rather than crashing. The legacy GL 2.1 / D3D10 backends are newer than the rest: world rendering works, the remaining passes (UI, particles, hand, sky) are being ported, and decoded world geometry is cached per draw so a steady view skips CPU decode and buffer re-upload.

## Running a server

`weft --server --world myworld --port 27182` runs a world with no window and no GPU. Forward the port, TCP and UDP, and players join with `host:port` in the Multiplayer screen, or with the ticket it logs. It keeps what a game keeps in the same folders as above, or in `--run-dir`.

Type `stop` to save and leave, `list`, `say <text>`, `op <player> [level]` or `deop <player>`. Nobody can use commands until you `op` them. To limit who may join, list their keys one a line in `config/whitelist.txt`: anyone refused is logged with the line to add.

## Contributing

Issues and pull requests are welcome, for the engine, mods, textures or docs. Read the [AI policy](AI_POLICY.md) before opening a pull request, it applies to everyone and the template asks for it.

## License - [MPL-2.0](LICENSE)

Made by [VortexSo4](https://github.com/VortexSo4). Legacy GPU support and this fork's builds by [MaazGAMER76](https://github.com/MaazGAMER76).
