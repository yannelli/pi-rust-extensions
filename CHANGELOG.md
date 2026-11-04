# Changelog

All notable changes to pi-rust-extensions are documented here. Versions follow [Semantic Versioning](https://semver.org/); before 1.0, breaking changes bump the minor version.

## [0.5.10] - 2026-11-04

### Added

- register the timers and simple-goal extensions ([d2036ba](https://github.com/yannelli/pi-rust-extensions/commit/d2036ba6daa1a9a8abe37d824692e85933cb9783))
- **typesafe:** list OpenRouter decision models with context and pricing ([0315342](https://github.com/yannelli/pi-rust-extensions/commit/0315342c31abe6d0c857d57ce9b4bbc4babd7712))
- **simple-goal:** add a single objective judged by TypeSafe or an LLM evaluator ([0370577](https://github.com/yannelli/pi-rust-extensions/commit/037057754cf10891adb3fad4e10d7176ad46bce5))
- **timers:** add sleep, heartbeat, and schedule tools ([493de34](https://github.com/yannelli/pi-rust-extensions/commit/493de3425dd93dccabb4150f1ee196fa287a2f88))
- **subagents:** add agent_message between agents and retry result delivery ([9167f15](https://github.com/yannelli/pi-rust-extensions/commit/9167f155d8ebd7e3236671ae272816f2a0b5fb42))
- **background-tasks:** emit agent:wake after a notification that starts a turn ([e1fb959](https://github.com/yannelli/pi-rust-extensions/commit/e1fb95967ac0e6560b2909e99c70eceb14cda3f9))
- **workflows:** fixed and dynamic multi-agent workflows ([ada1046](https://github.com/yannelli/pi-rust-extensions/commit/ada10461b5b249f27e64a68ceab640ca64150429))
- **subagents:** offer agent configs as deferrable context entries ([d8c83a9](https://github.com/yannelli/pi-rust-extensions/commit/d8c83a982f998575607921dcdcb926cc5452151b))
- **tool-results:** durable result store and expandable full results ([34b6bb2](https://github.com/yannelli/pi-rust-extensions/commit/34b6bb2faae35ff037f0ad09b0af96acf3a622ec))
- **claude-marketplace:** config overrides, plugin env, model rules, and skill sources ([c155321](https://github.com/yannelli/pi-rust-extensions/commit/c155321cade0dda47cef8c729a2ddaddb6a96705))
- **compact-tui:** border color, prompt layouts, and list overflow ([c3c07ff](https://github.com/yannelli/pi-rust-extensions/commit/c3c07ffefe9497b0accc98cd46c1dfce0ac08f0a))
- **tool-results:** add an extension that shrinks tool results ([fb3f6b0](https://github.com/yannelli/pi-rust-extensions/commit/fb3f6b08ed469f446790978adec859dde1236486))
- **claude-marketplace:** install output styles, bin, and user config of Claude plugins ([e86e6ba](https://github.com/yannelli/pi-rust-extensions/commit/e86e6ba1b6672e8a118bc81e1fc532d5922d7c58))

### Changed

- cut the prompt size of subagent, recall, web, marketplace, deferred and chrome tools ([59160da](https://github.com/yannelli/pi-rust-extensions/commit/59160daf03acef783395c68903452a0b6898ca60))
- cut the prompt size of todo, ask_user_question, background and goal tools ([7aaf2fa](https://github.com/yannelli/pi-rust-extensions/commit/7aaf2fa8ae8925b68eecafdec5bd412f2698dcdf))

### Fixed

- **subagents:** leave the agent file path out of context entries ([a7164c2](https://github.com/yannelli/pi-rust-extensions/commit/a7164c2096ce831143efd261e55a35b223b6b775))
- **subscription-router:** declare cached slot providers at initialize ([6116876](https://github.com/yannelli/pi-rust-extensions/commit/611687672f42012a2cbbe03c58daaa2e3acb8a73))
- **subagents:** report results in the shape Paseo's pi-subagents adapter reads ([3e8bd74](https://github.com/yannelli/pi-rust-extensions/commit/3e8bd743be0fd1bc7cb4e1aa44c01521a9085496))

## [0.5.9] - 2026-11-03

### Added

- **deferred:** user-only skills, repo-scoped rules, and MCP off ([c8d1cbe](https://github.com/yannelli/pi-rust-extensions/commit/c8d1cbedd0210230a0253518b2c704714f921cde))
- **statusline:** shade contextBar cells by part of the context ([e87dad4](https://github.com/yannelli/pi-rust-extensions/commit/e87dad4aff84af5c2d6006789127595a009039b6))

## [0.5.8] - 2026-11-03

### Added

- **statusline:** draw contextBar as 10 braille cells in 5% steps ([05e1aff](https://github.com/yannelli/pi-rust-extensions/commit/05e1affca76774fc1976b16c2a1af0c4611d7651))

## [0.5.7] - 2026-11-02

### Added

- **statusline:** draw contextBar with braille cells ([f069e57](https://github.com/yannelli/pi-rust-extensions/commit/f069e57b726e555e1c7591eb9807e0eed8dd235a))

## [0.5.6] - 2026-11-02

### Added

- **hooks:** run mcp_tool and agent hook handlers ([0cf443c](https://github.com/yannelli/pi-rust-extensions/commit/0cf443c1cf72ba82e767400d1df546c76ae69fe7))

## [0.5.5] - 2026-11-02

### Added

- **compact-tui:** add a compact, animated TUI extension ([07f1a74](https://github.com/yannelli/pi-rust-extensions/commit/07f1a741c41fd948d1804022d33da1e957e2da74))
- **sdk:** tool renderers for other tools, row spinners and refresh, theme, working indicator, and chat style ([f6a8a9b](https://github.com/yannelli/pi-rust-extensions/commit/f6a8a9bc02a70ed31683f2f64977a8929b27e312))
- **hooks:** match Claude hook regexes with JavaScript syntax ([f6ad551](https://github.com/yannelli/pi-rust-extensions/commit/f6ad551dc9376a846c41284e0e85516c2f91b8c0))

### Fixed

- **statusline:** use the colors of pi's theme and follow footer changes ([bd23d6c](https://github.com/yannelli/pi-rust-extensions/commit/bd23d6c5f23a9741df73745e37454a28447399e0))

## [0.5.4] - 2026-11-02

### Added

- **hooks:** send the missing Claude input fields and honor sessionTitle and interrupt ([d6e106c](https://github.com/yannelli/pi-rust-extensions/commit/d6e106c126286820a8e761a1068e069cbafdd4a6))

## [0.5.3] - 2026-11-02

### Added

- **hooks:** fire the remaining Claude hook events ([83f4208](https://github.com/yannelli/pi-rust-extensions/commit/83f4208ed797efb03130677e3bd17f54a2f098a8))

## [0.5.2] - 2026-11-02

### Fixed

- **hooks:** match the current Claude and Codex hooks docs ([f1fec65](https://github.com/yannelli/pi-rust-extensions/commit/f1fec658690d23df9ff45a5766029f5e1238e3d3))

## [0.5.1] - 2026-11-02

### Added

- **deferred:** add an /acp-style /deferred panel ([9f85ed6](https://github.com/yannelli/pi-rust-extensions/commit/9f85ed6d26a0f15a7433ce21c107be0e2831c118))
