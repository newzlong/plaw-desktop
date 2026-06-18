# ローカライズブリッジ: Sandboxing

このページは強化版ブリッジです。テーマの位置付け、原文セクション導線、実行時の注意点をまとめています。

英語版原文:

- [../../sandboxing.md](../../sandboxing.md)

## テーマ位置付け

- 分類: セキュリティと統制
- 深度: 強化ブリッジ（セクション導線 + 実行ヒント）
- 使い方: 構成を把握してから、英語版の規範記述に従って実施します。

## 原文セクションガイド

- [H2 · Architecture](../../sandboxing.md#architecture)
- [H2 · Shipped backends (Phase 0)](../../sandboxing.md#shipped-backends-phase-0)
- [H2 · Shipped: Windows kernel-level hardening (Phase 0 deep-dive — PR #77)](../../sandboxing.md#shipped-windows-kernel-level-hardening-phase-0-deep-dive--pr-77)
- [H2 · Shipped: Token Integrity Level (Phase 1 — PRs #87–#91)](../../sandboxing.md#shipped-token-integrity-level-phase-1--prs-8791)
- [H3 · Empirical IL compatibility envelope (PR #89 integration tests)](../../sandboxing.md#empirical-il-compatibility-envelope-pr-89-integration-tests)
- [H3 · Lens C Gatekeeper failure mode](../../sandboxing.md#lens-c-gatekeeper-failure-mode)
- [H2 · Shipped: Phase 1c.2 (Token IL output capture)](../../sandboxing.md#shipped-phase-1c2-token-il-output-capture)
- [H2 · Decided against: BrowserTool Token IL (REJECTED, not deferred)](../../sandboxing.md#decided-against-browsertool-token-il-rejected-not-deferred)
- [H2 · Roadmap: not in scope today](../../sandboxing.md#roadmap-not-in-scope-today)
- [H2 · Reading source](../../sandboxing.md#reading-source)
- [H2 · Reading config](../../sandboxing.md#reading-config)

## 実行ヒント

- まず原文の見出し構成を確認し、今回の変更範囲に直結する節から読みます。
- コマンド名、設定キー、API パス、コード識別子は英語のまま保持します。
- 仕様解釈に差分が出る場合は英語版原文を優先します。

## 関連エントリ

- [README.md](README.md)
- [SUMMARY.md](SUMMARY.md)
- [docs-inventory.md](docs-inventory.md)
