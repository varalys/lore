# Lore: Reasoning History for Code

## The Problem

Traditional version control captures **code history**—what changed, when, and by whom. But in AI-assisted development, the actual story of how code came to be happens in conversations, iterations, and reasoning that git never sees.

When you review a PR today, you see:
- The diff
- The commit message
- Maybe a PR description

What you don't see:
- The sessions (possibly spanning days) that led to this code
- The prompts used to generate or refine it
- The dead ends and rejected approaches
- Why approach A was chosen over approach B
- What the AI suggested that was rejected
- The full collaboration between developer and AI

**Git = Code History**
**Lore = Reasoning History**

They're complementary. Git tells you *what* changed. Lore tells you *how* and *why* it changed—the collaboration between developer and AI that produced the code.

## Why Now

AI-assisted development makes this both necessary and newly possible:

- **Necessary** because reasoning now happens in conversations that aren't captured anywhere. Before LLMs, reasoning lived in a developer's head (also lost, but at least they could be asked). Now it's in ephemeral chat windows across multiple tools.

- **Possible** because those conversations are text. They're structured. They can be captured, linked, and made searchable. You couldn't record what was in someone's head, but you can record their dialogue with an AI.

The scale of AI-generated code is accelerating:
- 30% of Microsoft's code is now AI-generated
- 25%+ of Google's new code is AI-generated  
- Some YC W25 startups report 95% AI-generated codebases

The commit message "fixed bug" has always been inadequate. When an LLM conversation spanning 47 back-and-forths led to that fix, losing that context is painful.

## The Core Concept

**The session is the missing primitive, not a better commit.**

Git's unit of work is the commit. But in AI-assisted development, the real unit of work is the *session*—the full arc from "I need to build X" through prompts, iterations, dead ends, and refinements to working code.

Lore captures reasoning history as **sessions**—complete records of human-AI collaboration that can be linked to commits and PRs.

### Mental Model

Every commit/PR has a "development history" attached—not just code history (which git provides), but *reasoning history*. The sessions that contributed to this change.

The workflow:
1. Developer works normally across whatever AI tools they use
2. Sessions are captured automatically in the background
3. When they commit or open a PR, relevant sessions are associated with it
4. Reviewer opens the PR and can "expand" to see the full development narrative

The commit message becomes a summary, with the full documentary available when needed.

## What Lore Captures

A reasoning session includes:
- **Prompts**: What the developer asked
- **Responses**: What the AI suggested
- **Iterations**: The back-and-forth refinement
- **Tool calls**: What actions the AI took (file edits, searches, commands)
- **Rejections**: Suggestions that were discarded (when detectable)
- **Context**: Files referenced, documentation consulted
- **Metadata**: Model used, timestamps, tool/IDE

## Use Cases

### Code Review
Reviewer opens a PR and sees not just the diff, but the reasoning that produced it. They understand the "why" not just the "what."

### Debugging / Archaeology  
"Why was this written this way?" Instead of guessing or finding the original author, you can see the exact conversation that led to this code.

### Onboarding
New team members can see how code was actually built, not just what it looks like now. They learn the team's problem-solving patterns.

### Knowledge Retention
When someone leaves, you have their reasoning, not just their code. The "why" doesn't walk out the door.

### Team Learning
What prompts work well? What approaches fail? Teams can learn from each other's AI interactions.

### Compliance / Audit
As AI code provenance becomes legally relevant (EU AI Act, copyright concerns), Lore provides documentation of how code was developed.

## Relationship to Git

Lore sits alongside git, not replacing it.

- **Reasoning sessions** exist independently (captured automatically as you work)
- **Commits/PRs** exist independently (git does its thing)  
- **Links** connect them (automatic based on timing/files, or developer-curated)

This keeps the system flexible without forcing developers to change how they commit.

### Storage Philosophy

Full reasoning history lives in Lore's system (local-first, syncs to cloud). Pointers can be pushed into git as notes or commit message footers:

```
fix: resolve race condition in queue processor

Lore-Sessions: lore://session/abc123, lore://session/def456
```

The full story lives in Lore. The breadcrumb is in git. Anyone with Lore access can follow the link.

## Market Context

### What Exists Today

**AI coding tools** have limited history:
- **Claude Code**: Stores conversations in JSONL files, but no git integration
- **Cursor**: SQLite databases with conversation history, but local-only and fragile
- **GitHub Copilot**: 30-day retention, no automatic commit linking
- **Replit**: Best integration (checkpoints + context), but not exportable

**No major tool automatically creates a provenance chain linking AI conversation → code changes → git commit.**

### Emerging Players

- **YOYO** (runyoyo.com): "AI version control for vibe coding"—checkpoint/restore focus, 8k+ users
- **Tabnine Provenance**: Enterprise feature for code attribution (December 2024)
- **git-ai**: Open source tool using git notes for AI attribution per line

### The Gap

No one owns the **connective tissue between AI interactions and code artifacts**—capturing what git misses without disrupting the flow that makes AI-assisted development productive.

## Competitive Positioning

YOYO is the closest competitor but positions as "undo AI mistakes"—a recovery tool. That's a feature, not a platform.

Lore positions as **reasoning history**—the complete development narrative that makes code understandable, reviewable, and auditable.

## Target Users

**Primary**: Development teams using AI-assisted coding who need to:
- Review each other's AI-generated code effectively
- Maintain institutional knowledge as people move around
- Understand codebases they didn't write

**Secondary**: Individual developers who want to:
- Remember why they wrote something weeks later
- Search their own development history
- Build a personal knowledge base of effective AI interactions

**Enterprise**: Organizations with:
- Compliance requirements around AI-generated code
- Audit needs for code provenance
- Knowledge management concerns

## Success Metrics

- Sessions captured per developer per day
- Sessions linked to commits (automatic vs manual)
- Session views during code review
- Search queries against reasoning history
- Time to understand unfamiliar code (before/after)

## Open Questions

1. **Cross-tool capture**: Can we reliably capture from Cursor, Claude Code, Copilot, and others? This is technically hard but essential for the value prop.

2. **Automatic vs manual linking**: How much can we automatically associate sessions with commits vs requiring developer action?

3. **Privacy/sensitivity**: Reasoning sessions may contain sensitive information (API keys in prompts, proprietary logic discussions). How do we handle this?

4. **Team dynamics**: When is reasoning history shared vs private? Per-session? Per-repo?

5. **Storage scale**: Reasoning history is verbose. What's the storage/cost model?

## MVP Scope

Start narrow to prove the core value:

1. **Single tool**: Claude Code (transparent JSONL storage, Chris uses it)
2. **Auto-capture**: Sessions recorded automatically in background
3. **Manual linking**: CLI to link sessions to commits: `lore link <session> --commit HEAD`
4. **Simple viewer**: `lore show --commit HEAD` displays reasoning history for that commit
5. **Local-first**: No cloud sync initially

Prove: Can you look at a commit and see the reasoning that produced it? Is that valuable?

Then expand: Cross-tool capture, automatic linking, team sync, GitHub/GitLab integration.
