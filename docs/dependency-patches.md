# Dependency patches

## Kobalte scroll traversal

`@kobalte/utils@0.9.2` is patched through `pnpm-workspace.yaml`; the lockfile records the patch hash. Frozen installs apply the same patch to the source and both ESM and CommonJS runtime builds. No dependency versions are upgraded by this patch.

Opening the Skill actions menu after creating a Skill reproduced a browser freeze with both the real production page and native desktop E2E. Pausing Chromium showed `scrollIntoViewport` repeatedly calling `getScrollParent` on the same scrollable container while the document root had `overflow: hidden`. The helper includes its argument in the search, so that loop did not make progress. The [upstream implementation](https://github.com/kobaltedev/kobalte/blob/main/packages/utils/src/scroll-into-view.ts) is the dependency source reference.

After scrolling a container, the patch continues the search at its parent element. This preserves overlay scroll locking, the scrollable menu and keyboard navigation while ensuring traversal reaches the document root.

The production browser regressions create an editable Skill, open its menu with pointer and keyboard input, open the new-file dialog, close it and switch settings pages. The existing native test still covers actual SkillHost creation, editing, conflicts and persistence. Remove the patch only after an upgraded dependency passes these regressions.
