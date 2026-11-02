# Notes for AI coding agents

- Read [CONTRIBUTING.md](CONTRIBUTING.md) (where code goes, privacy and behaviour rules) and
  [docs/architecture.md](docs/architecture.md) before changing anything.
- Load the skill `.claude/skills/lumen-motion/SKILL.md` before touching any animation, transition,
  hover/press state, popover or page change.
- Logic goes in Rust (`src/`); `browser/bridge.js` stays generic.
- Build with `./build.sh` (public) or `./build.sh --dev` (development); test with `npm test`.
- Commit only the public build (`./build.sh && npm run check:public`). Never commit incident files,
  screenshots of real accounts, personal paths or real IPs/visitor data.
