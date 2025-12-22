# Lore: Business Model

## Strategic Approach: Open Core

Lore follows an open core model where the complete CLI and local functionality are open source, with cloud services and team features as the monetization layer.

### Why Open Source First

**Trust**: Lore captures AI conversations—potentially sensitive prompts, proprietary logic, API keys. Open source lets users verify exactly what's being captured and stored. This is a meaningful trust signal for a tool that sees everything.

**Adoption**: Dev tools live or die on adoption. A free, open core removes friction. Developers can try it without a credit card or sales call. This is how git, VS Code, and most successful dev tools spread.

**Community contributions**: Capture across Cursor, Copilot, Windsurf, and future tools is significant surface area. Community contributions can accelerate watcher development and maintenance.

**Low defensibility concern**: The value isn't in the CLI code—it's in the network effects of teams using it together and the cloud infrastructure. Someone can fork the CLI, but they can't fork the user base or team adoption.

---

## Market Segments

| Segment | Why They Pay | Price Sensitivity | Sales Motion |
|---------|--------------|-------------------|--------------|
| Individual developers | Convenience, backup | High | Self-serve |
| Teams (5-50 devs) | Collaboration, knowledge sharing | Medium | Self-serve + light touch |
| Enterprise (50+ devs) | Compliance, audit, control | Low | Sales-led |

**Primary target**: Teams of 5-50 developers at companies using AI-assisted development heavily. They have budget, can make decisions quickly, and get immediate value from shared reasoning history.

**Long-term target**: Enterprise, but only after product-market fit with teams. Enterprise sales is slow and shouldn't distract from early traction.

---

## Pricing Tiers

### Open Source — Free Forever

```
┌─────────────────────────────────────────────────────────────────┐
│  OPEN SOURCE (MIT or Apache 2.0)                                │
│─────────────────────────────────────────────────────────────────│
│                                                                 │
│  • Full capture from all supported tools                        │
│  • Local SQLite storage                                         │
│  • Complete CLI (sessions, show, link, search)                  │
│  • Git integration (hooks, auto-linking)                        │
│  • Works offline, forever, no account needed                    │
│                                                                 │
│  This is the product. Not a crippled demo.                      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Principle**: The open source version must be genuinely useful on its own. Someone who never pays should get real value. This builds trust and drives adoption.

---

### Free Cloud — $0 (with account)

```
┌─────────────────────────────────────────────────────────────────┐
│  FREE CLOUD                                                     │
│─────────────────────────────────────────────────────────────────│
│                                                                 │
│  Everything in Open Source, plus:                               │
│                                                                 │
│  • Cloud backup of sessions                                     │
│  • Sync across machines                                         │
│  • 90-day retention                                             │
│  • 1 GB storage                                                 │
│  • Web viewer (read-only)                                       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Purpose**: Convert open source users to accounts. Get email addresses. Introduce cloud value prop. Create upgrade path.

**Limits rationale**:
- 90-day retention: Long enough to be useful, short enough to create upgrade pressure for power users
- 1 GB storage: Enough for casual use, not enough for heavy AI-assisted development over time

---

### Pro — $8/month

```
┌─────────────────────────────────────────────────────────────────┐
│  PRO                                                            │
│─────────────────────────────────────────────────────────────────│
│                                                                 │
│  Everything in Free Cloud, plus:                                │
│                                                                 │
│  • Unlimited retention                                          │
│  • 50 GB storage                                                │
│  • Priority support                                             │
│  • Early access to new watchers                                 │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Target**: Individual developers who use AI coding heavily and want their full history backed up and searchable.

**Pricing rationale**: $8/month is impulse-purchase territory for developers. Lower than Cursor ($20), comparable to other dev tools. Not trying to maximize revenue here—trying to build habit and identify enthusiasts who might bring Lore to their teams.

---

### Team — $15/seat/month

```
┌─────────────────────────────────────────────────────────────────┐
│  TEAM                                                           │
│─────────────────────────────────────────────────────────────────│
│                                                                 │
│  Everything in Pro, plus:                                       │
│                                                                 │
│  • Share sessions with teammates                                │
│  • GitHub/GitLab PR integration                                 │
│    - View reasoning history directly in PRs                     │
│    - Link sessions from PR comments                             │
│  • Team dashboard                                               │
│    - AI usage patterns                                          │
│    - Effective prompt analysis                                  │
│    - Session activity overview                                  │
│  • Centralized billing and admin                                │
│  • Team-level privacy controls                                  │
│                                                                 │
│  Minimum 5 seats                                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Target**: Development teams who want the code review and knowledge sharing benefits.

**Pricing rationale**: $15/seat is competitive with collaboration tools (Linear $8, Notion $10, GitHub Team $4). The value prop—better code review, preserved institutional knowledge—justifies premium over basic project management tools.

**5-seat minimum**: Filters out individuals trying to get team features cheap. Team value requires... a team.

**Key features that drive upgrades**:
- PR integration is the killer feature. Seeing reasoning history during code review is the "aha" moment.
- Team analytics create visibility for engineering managers.

---

### Enterprise — Custom Pricing

```
┌─────────────────────────────────────────────────────────────────┐
│  ENTERPRISE                                                     │
│─────────────────────────────────────────────────────────────────│
│                                                                 │
│  Everything in Team, plus:                                      │
│                                                                 │
│  • Self-hosted deployment option                                │
│  • SSO / SAML authentication                                    │
│  • Advanced audit logs                                          │
│  • Data residency controls                                      │
│  • Compliance certifications (SOC 2, etc.)                      │
│  • Custom retention policies                                    │
│  • Dedicated support                                            │
│  • SLA guarantees                                               │
│  • Custom integrations                                          │
│                                                                 │
│  Annual contracts, typically $30-50/seat/month                  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Target**: Large organizations with compliance requirements, security concerns, or need for on-premise deployment.

**Timing**: Don't build enterprise features until there's inbound demand. These deals are slow and distract from product-market fit.

**Self-hosted**: Important for companies that can't have AI conversation data in third-party cloud. Premium feature, significant price multiplier.

---

## Comparable Pricing

| Product | Individual | Team | Notes |
|---------|------------|------|-------|
| GitHub | Free | $4/seat | Massive scale, low price anchor |
| Linear | $8/seat | $8/seat | Single tier simplicity |
| Notion | $8/seat | $10/seat | Slight team premium |
| Cursor | $20/month | $40/seat | AI-native, higher price tolerance |
| PostHog | Free | Usage-based | Open source + cloud |
| Sentry | Free | $26/seat | Developer infrastructure |

Lore's $15/seat for Team is in the middle of the range—higher than GitHub (different value prop) but lower than Cursor (which is the core coding tool, not supplementary).

---

## Revenue Model Dynamics

### Funnel

```
Open Source Users (free)
    │
    │ Cloud signup (email capture)
    ▼
Free Cloud Users (0$ - 90 day retention)
    │
    │ Hit retention/storage limits OR want backup
    ▼
Pro Users ($8/month)
    │
    │ Bring to team, want collaboration
    ▼
Team Users ($15/seat/month)
    │
    │ Scale + compliance needs
    ▼
Enterprise ($30-50/seat/month)
```

### Key Metrics to Track

**Top of funnel**:
- GitHub stars
- CLI downloads
- Active open source users (telemetry opt-in)

**Conversion**:
- Open source → Free Cloud signup rate
- Free Cloud → Pro conversion rate
- Pro → Team expansion rate

**Retention**:
- DAU/MAU for CLI usage
- Session capture volume per user
- Sessions viewed during code review (Team)

**Revenue**:
- MRR / ARR
- Average revenue per team
- Net revenue retention (expansion - churn)

---

## Phased Rollout

### Phase 1: Open Source Only (Months 1-3)

- Ship open source CLI
- Build community
- Gather feedback on core value prop
- No monetization yet

**Goal**: Prove that reasoning history is valuable. Get developers using it daily.

### Phase 2: Cloud Beta (Months 4-6)

- Launch Free Cloud tier
- Collect email addresses
- Test sync/backup value prop
- Soft launch Pro tier

**Goal**: Validate that cloud adds value beyond local. Build infrastructure.

### Phase 3: Team Launch (Months 7-9)

- Ship GitHub/GitLab PR integration
- Launch Team tier
- Build team dashboard
- First paying team customers

**Goal**: Find product-market fit with teams. Validate $15/seat price point.

### Phase 4: Scale (Months 10+)

- Enterprise features (as demanded)
- Self-hosted option
- Expand integrations
- Grow team sales

**Goal**: Repeatable sales motion. Path to $1M ARR.

---

## Financial Projections (Illustrative)

### Year 1 Target

| Metric | Target |
|--------|--------|
| Open source users | 5,000 |
| Free Cloud users | 1,000 |
| Pro subscribers | 100 |
| Team seats | 50 |
| MRR | $1,550 |
| ARR | ~$19,000 |

### Year 2 Target

| Metric | Target |
|--------|--------|
| Open source users | 25,000 |
| Free Cloud users | 5,000 |
| Pro subscribers | 500 |
| Team seats | 500 |
| MRR | $11,500 |
| ARR | ~$138,000 |

These are conservative estimates. The key assumption is that reasoning history provides enough value during code review that teams will pay for the integration.

---

## Risks and Mitigations

### Risk: AI coding tools build this natively

**Likelihood**: Medium-high. Cursor, Claude Code could add reasoning history themselves.

**Mitigation**: 
- Cross-tool is the moat. We capture from all tools; they only have their own data.
- Move fast. Establish as the standard before tools fragment.
- Open source builds loyalty and switching costs.

### Risk: Low willingness to pay

**Likelihood**: Medium. Developers are used to free tools.

**Mitigation**:
- Heavy free tier reduces friction
- Team/enterprise is where money is, not individuals
- PR integration is differentiated value, not "nice to have"

### Risk: Privacy concerns limit adoption

**Likelihood**: Low-medium. Some orgs won't want AI convos captured.

**Mitigation**:
- Open source builds trust (auditable)
- Self-hosted option for enterprise
- Strong privacy controls and redaction features

### Risk: GitHub/GitLab builds this

**Likelihood**: Low-medium. They have the PR integration surface area.

**Mitigation**:
- They move slowly
- They'd likely only integrate with Copilot, not cross-tool
- Acquisition target if they want to buy vs build

---

## Open Questions

1. **License choice**: MIT (maximum adoption) vs Apache 2.0 (patent protection) vs AGPL (forces cloud competitors to open source)?

2. **Telemetry**: Should the open source CLI phone home for usage stats? Trade-off between data and trust.

3. **Pro tier necessity**: Could we skip Pro and go straight to Free → Team? Simpler pricing, but lose individual revenue.

4. **Annual discounts**: Standard practice is 2 months free for annual. Worth the cash flow trade-off early on?
