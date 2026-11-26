# Changelog

All notable changes to pi-rust-extensions are documented here. Versions follow [Semantic Versioning](https://semver.org/); before 1.0, breaking changes bump the minor version.

## [0.5.28] - 2026-11-25

### Added

- ssh bash routing tools, Windows ssh, and a port for launch_browser (#84) ([65ef865](https://github.com/yannelli/pi-rust-extensions/commit/65ef865b3b0f461351d2d1b892655b49697e5fd2))
- **subagents:** load agents from the folders other pi subagent extensions use ([e0dd9b2](https://github.com/yannelli/pi-rust-extensions/commit/e0dd9b270cbc45112cc62e4dc978045232767fe8))

## [0.5.27] - 2026-11-25

### Fixed

- **plan-mode:** point the hidden tool advice at deferred rules ([ec92e30](https://github.com/yannelli/pi-rust-extensions/commit/ec92e30f6dc5dca050f56020c3da7eb73b2ca0dd))
- **deferred:** keep plan mode tools on by default ([36ea73e](https://github.com/yannelli/pi-rust-extensions/commit/36ea73e6685c573a41f506bc646104a296d193b6))

## [0.5.26] - 2026-11-25

### Added

- **plan-mode:** keep the plan tools visible and warn about tools the agent cannot use ([52d52bb](https://github.com/yannelli/pi-rust-extensions/commit/52d52bb668fabfc7e14d7ad8679feb5f5d7f976e))
- **plan-mode:** add codex-like /plan mode ported from @narumitw/pi-plan-mode ([d8fe1dc](https://github.com/yannelli/pi-rust-extensions/commit/d8fe1dc730b20b33ea0837b4780b40bee4dc75d0))

### Fixed

- **plan-mode:** check tools at call time and limit only typed answers ([b64eaef](https://github.com/yannelli/pi-rust-extensions/commit/b64eaef0438fe8a10b17647f4d91bfd6a78de057))

## [0.5.25] - 2026-11-24

### Fixed

- **subscription-router:** remove temp files that killed writers left ([884c5f0](https://github.com/yannelli/pi-rust-extensions/commit/884c5f04b4356b795a493301ab8533a3ee18a87b))
- **workflows:** make runs reliable and rebuild the /workflows panel ([de3cb47](https://github.com/yannelli/pi-rust-extensions/commit/de3cb47283130a0b91fcfe7dc6b0b17c76cfaaf1))

## [0.5.24] - 2026-11-23

### Added

- **iterm2:** add /iterm2 panel with tab color themes and per-state colors (#79) ([56ae981](https://github.com/yannelli/pi-rust-extensions/commit/56ae981d4bc8f6ea2cb164a0fc9a0dd79f2be7f8))

## [0.5.23] - 2026-11-22

### Fixed

- **hooks:** show conflict tags only under warn and override ([7be1016](https://github.com/yannelli/pi-rust-extensions/commit/7be1016cfdc9eab2e8adeed251ca5f9fecbb77cf))

## [0.5.22] - 2026-11-22

### Fixed

- **web-search:** send Claude Code 2.1.322 on Anthropic OAuth searches ([25e93c3](https://github.com/yannelli/pi-rust-extensions/commit/25e93c3f5d89835cd24299cd35ab56c16091de73))

## [0.5.21] - 2026-11-21

### Added

- **hooks:** load Codex plugin, MDM, cloud cache, and managed_config.toml hooks ([7737076](https://github.com/yannelli/pi-rust-extensions/commit/7737076574eb9577107c08d46e2a98bf6304ac12))
- **hooks:** read Claude managed policy from MDM, the registry, and the server-managed cache ([f6ba44a](https://github.com/yannelli/pi-rust-extensions/commit/f6ba44a2bbaf9d6c4633bd13feda7537f555403e))
- **hooks:** add Claude Code's prompt and agent hook limit behind a switch ([20e1f5e](https://github.com/yannelli/pi-rust-extensions/commit/20e1f5e07510288524fcaaa07f31898cab94d395))
- **hooks:** support Codex onFailure behind the codex/onFailure switch ([e8ccba4](https://github.com/yannelli/pi-rust-extensions/commit/e8ccba4785e0f0a1ce4233a31a8cc5fa1ccc95ee))
- **hooks:** send PreCompact stdout as compaction instructions ([648c5db](https://github.com/yannelli/pi-rust-extensions/commit/648c5db268077a175ae6c53ed080c4baab66c75c))
- **hooks:** run force-enabled plugin hooks under managed hook gates ([0e96f4b](https://github.com/yannelli/pi-rust-extensions/commit/0e96f4b38ab397bd302b4daade489dbb762114d5))
- **hooks:** add feature switches to the /hooks Settings tab ([a374b89](https://github.com/yannelli/pi-rust-extensions/commit/a374b89f8ee4d10b244e147b18b4bbdd36784c73))
- **hooks:** merge hooks by layer, load managed hooks, and match the Nov 2026 docs ([2950246](https://github.com/yannelli/pi-rust-extensions/commit/29502460bf9fd8a3f458c3e8eebb639a6fe3c360))

### Fixed

- **hooks:** keep admin locks when the server copy picks first-wins ([9ea7350](https://github.com/yannelli/pi-rust-extensions/commit/9ea735043612766d77cc3d5cd23d98cba8fbc33c))
- **hooks:** turn off all Codex hooks when a root source sets hooks = false ([66a7021](https://github.com/yannelli/pi-rust-extensions/commit/66a7021c93a3ccb5e63b5d178aeac4d7b0a63a95))
- **hooks:** fix review findings in the stacked hooks changes ([c946afd](https://github.com/yannelli/pi-rust-extensions/commit/c946afdb36a06dee4f8f7a3736ab021b44f53ba8))
- **hooks:** gate PreCompact instructions on the compactionInstructions host feature ([181f36c](https://github.com/yannelli/pi-rust-extensions/commit/181f36c66327aaf542ec8109e22733209cb97e60))
- **hooks:** apply stricter cloud requirements and keep plugin hooks disabled ([75c42c5](https://github.com/yannelli/pi-rust-extensions/commit/75c42c5062cd6a632f6080bb7270125e13c9a3e7))
- **hooks:** ask for trust on user-writable managed hooks and limit plutil and reg ([dff2958](https://github.com/yannelli/pi-rust-extensions/commit/dff2958eec22fa6f614b6c419732258f0ae3423d))
- **hooks:** load the Codex cloud cache as user hooks and recheck plugin files ([3fd64af](https://github.com/yannelli/pi-rust-extensions/commit/3fd64afc72cb88ee6b71eb9182e6ecc562691983))
- **hooks:** say when the signature file moves and that pi skips the hook approval ([f745487](https://github.com/yannelli/pi-rust-extensions/commit/f74548742bb001bc8221e74f6bfc27644b1eb53d))
- **hooks:** match Claude Code on the server cache age, merged disableAllHooks, and the plist user ([8f46a72](https://github.com/yannelli/pi-rust-extensions/commit/8f46a7274d67962bc1e4bab55723d0db7a994db4))
- **hooks:** run Codex mcp_tool and asyncRewake hooks in the foreground ([41747ae](https://github.com/yannelli/pi-rust-extensions/commit/41747ae0dbd6f660f4f85a631250c438b6f4c83d))
- **hooks:** fail Codex hookSpecificOutput without a valid hookEventName ([d00d7d5](https://github.com/yannelli/pi-rust-extensions/commit/d00d7d55d2e15ce03fb348b25dbbada5497fa2ff))
- **hooks:** match Codex inline hook shapes and skip Codex-only plugin hooks ([2aa3257](https://github.com/yannelli/pi-rust-extensions/commit/2aa325740de89cad4dc5d829510a66df023ec750))
- **hooks:** apply managed HTTP allowlists with the Claude source off ([27ca1ab](https://github.com/yannelli/pi-rust-extensions/commit/27ca1ab0985a9876fdfb8a3271b104082f43f750))
- **hooks:** keep async prompt and agent hooks out of the hook limit ([1a7e5c7](https://github.com/yannelli/pi-rust-extensions/commit/1a7e5c72b9494e7b604c4e4d8492b6cd9c0f9b1a))
- **hooks:** apply Claude managed plugin gates with the Claude source off ([cd18694](https://github.com/yannelli/pi-rust-extensions/commit/cd18694bea4f4cf9d6c2f4db8ebb47389a1c7dae))
- **hooks:** correct the force-enabled plugin switch text and limits ([7612fdf](https://github.com/yannelli/pi-rust-extensions/commit/7612fdf568add2b9952adbf7ddb495dab6776ad2))

## [0.5.20] - 2026-11-19

### Fixed

- **hooks:** run shell-form hooks with Git Bash on Windows ([0c5650d](https://github.com/yannelli/pi-rust-extensions/commit/0c5650dba98caec7771529f70ed37ab538264d2b))

## [0.5.19] - 2026-11-19

### Added

- **background-tasks:** tasks survive pi restarts in RPC mode and re-attach ([a1b9edc](https://github.com/yannelli/pi-rust-extensions/commit/a1b9edc3bdd6ebf173223d56849cf9f14b1776fe))

### Changed

- **sdk:** ask pi for the boundary context only when a handler reads it ([00212f8](https://github.com/yannelli/pi-rust-extensions/commit/00212f8f4c103bdf50aa2452983f5739f1985e91))

## [0.5.18] - 2026-11-19

### Added

- **subagents:** background runs survive pi restarts and re-attach ([cde8602](https://github.com/yannelli/pi-rust-extensions/commit/cde8602466a0f07e3d258f98e8529090cc4c9861))

## [0.5.17] - 2026-11-19

### Fixed

- **goal:** add the pi-goal-x details to goal tool results ([8755593](https://github.com/yannelli/pi-rust-extensions/commit/87555934d653ad19494403b9b61e260065346524))
- **todo:** report update as the details action of added actions ([b633bb7](https://github.com/yannelli/pi-rust-extensions/commit/b633bb7f7f41190ecbb45546755c4d9cf5029db3))
- **subagents:** match the Paseo subagent contracts ([7e5d787](https://github.com/yannelli/pi-rust-extensions/commit/7e5d787acf25ee05ed85bf30a616b9ad70521ff8))

## [0.5.16] - 2026-11-18

### Added

- **subscription-router:** /router menu, model access per account, indicator settings ([5c34b2c](https://github.com/yannelli/pi-rust-extensions/commit/5c34b2c172b0a8f672206c7e9f21f0d0dda78069))

## [0.5.15] - 2026-11-11

### Added

- **hybrid-router:** route prompts between a local and a remote model with /hybrid ([0812b3d](https://github.com/yannelli/pi-rust-extensions/commit/0812b3d90ae829bb54bb09c294cbf9b31f0af6b4))
- **claude-marketplace:** a /claude-plugins panel like /deferred's ([589d313](https://github.com/yannelli/pi-rust-extensions/commit/589d313f17ef0acbb0a1dc8c5ee4939f262d3e3e))
- **subscription-router:** manage accounts without dialogs ([8be954f](https://github.com/yannelli/pi-rust-extensions/commit/8be954fb7c09f3cb7b1ad246cbaa407cf454e5c2))
- **subscription-router:** accounts behind one provider, picked per request ([2a44409](https://github.com/yannelli/pi-rust-extensions/commit/2a44409e999359e91ffe777752f59626d81c4cc5))

## [0.5.14] - 2026-11-04

### Added

- read project config from the main checkout in git worktrees ([727fb55](https://github.com/yannelli/pi-rust-extensions/commit/727fb550409561914466a11f90ea4f9714a72591))
- **typesafe:** review and block tool calls against custom rules ([401d176](https://github.com/yannelli/pi-rust-extensions/commit/401d176de2ad174a35ad9957021e9f00e09989c3))

## [0.5.13] - 2026-11-04

### Added

- **workflows:** live agent output in the run view and steering with m ([bcb7cf8](https://github.com/yannelli/pi-rust-extensions/commit/bcb7cf840bdf21fb9df32220948651ce1e10a9ed))

## [0.5.12] - 2026-11-04

### Added

- **subagents:** rework the /agents panel ([10be225](https://github.com/yannelli/pi-rust-extensions/commit/10be2254c89cbb8605f685dc0dfa3ace74824a1a))
- **claude-marketplace:** install LSP servers of Claude plugins into lens ([76e6fae](https://github.com/yannelli/pi-rust-extensions/commit/76e6faea4e0ad564bbf0b2b1262f53fd51bdcb99))
- **lens:** run LSP servers for any file extension from server files ([f3ba3eb](https://github.com/yannelli/pi-rust-extensions/commit/f3ba3eb1243f23158f44f0888eb3c2a4e175924b))

## [0.5.11] - 2026-11-04

### Added

- **workflows:** background runs and approvals that wait for the user ([2e6257b](https://github.com/yannelli/pi-rust-extensions/commit/2e6257b2011b402caed3df5b2f97ff0c525071f8))

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
