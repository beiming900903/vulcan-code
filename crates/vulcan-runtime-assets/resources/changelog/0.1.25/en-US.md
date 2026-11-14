Vulcan Code 0.1.25 improves desktop startup, knowledge bases, and dedicated assistants. The standalone setup program 0.2.13 adds a choice of download routes.

### Startup and runtime

```ui-checklist
ok | Desktop startup progress | The startup screen shows the current Core stage and names the skill being downloaded.
ok | LuaSkills compatibility | Core uses the bundled Lua runtime and module paths without interference from inherited Lua environment variables. Lua modules can load from Windows installation paths with non-ASCII characters.
```

### Knowledge bases

```ui-checklist
ok | Reuse indexes | Switching search modes preserves existing indexes and embedding settings. Re-enabling the same vector space fills in only the missing content.
ok | Document previews and search | The web app previews current and historical documents in Markdown, with an option to view the source. Search summaries open at the beginning of the document. Text matches still open at the matching line.
```

### Assistants and models

```ui-checklist
ok | Locked memory | Converting a session opens the floating assistant and shows its lock status. Turns completed under the lock are marked so you can see which content will be left out of the next turn.
ok | System prompts | Changes to the base system prompt take effect on the next execution, before conversation history is sent, without refreshing the session.
```

### Standalone setup 0.2.13

```ui-checklist
ok | Download routes | Choose Automatic, International, or Mainland China when installing or updating. Setup remembers the choice, and you can set it with `--route`.
```
