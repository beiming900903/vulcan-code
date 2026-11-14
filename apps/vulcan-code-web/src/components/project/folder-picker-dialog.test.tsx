// @vitest-environment jsdom
import { act, type ReactNode } from "react"
import { createRoot } from "react-dom/client"
import { describe, expect, it, vi } from "vitest"
import { I18nProvider } from "@/i18n"
import type { ProjectDirectoryListing } from "@/types"
import { ProjectFolderPickerDialog, projectFolderPickerCrumbs, projectFolderPickerPlaces } from "./folder-picker-dialog"

vi.mock("@/ui-kit/primitives/dialog", () => ({
  Dialog: (props: { open: boolean; children: ReactNode }) => props.open ? <div>{props.children}</div> : null,
  DialogContent: (props: { children: ReactNode }) => <div>{props.children}</div>,
  DialogDescription: (props: { children: ReactNode }) => <p>{props.children}</p>,
  DialogFooter: (props: { children: ReactNode }) => <footer>{props.children}</footer>,
  DialogHeader: (props: { children: ReactNode }) => <header>{props.children}</header>,
  DialogTitle: (props: { children: ReactNode }) => <h2>{props.children}</h2>,
}))

// TREE is a synthetic Windows host: one drive, a projects folder with two children and a dot folder.
// TREE 是合成的 Windows 宿主：一个盘符，一个含两个子目录与一个点目录的 projects 目录。
const TREE: Record<string, string[]> = {
  "C:\\": ["Users"],
  "C:\\Users": ["dev"],
  "C:\\Users\\dev": ["projects"],
  "C:\\Users\\dev\\projects": ["alpha", "beta", ".cache"],
  "C:\\Users\\dev\\projects\\alpha": [],
  "C:\\Users\\dev\\projects\\beta": [],
  "C:\\Users\\dev\\projects\\.cache": [],
}

// listingOf builds the service listing of one synthetic directory.
// listingOf 构建一个合成目录的服务列表。
function listingOf(path: string): ProjectDirectoryListing {
  const children = TREE[path]
  if (!children) throw new Error(`missing ${path}`)
  const crumbs = projectFolderPickerCrumbs(path)
  return {
    path,
    parentPath: crumbs.length > 1 ? crumbs[crumbs.length - 2].path : null,
    canSelect: true,
    roots: [{ label: "C:", path: "C:\\" }],
    entries: children.map((name) => ({ name, path: path.endsWith("\\") ? `${path}${name}` : `${path}\\${name}`, hidden: name.startsWith(".") })),
  }
}

// openPicker renders the picker at the projects folder and returns its spies and helpers.
// openPicker 在 projects 目录渲染选择器，并返回监听函数与辅助方法。
async function openPicker(projectPaths: string[] = []) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
  const container = document.createElement("div")
  const root = createRoot(container)
  document.body.append(container)
  const browse = vi.fn(async (path: string | null) => listingOf(path ?? "C:\\Users\\dev\\projects"))
  const confirm = vi.fn(async () => undefined)
  await act(async () => root.render(<I18nProvider initialLocale="en-US">
    <ProjectFolderPickerDialog open projectPaths={projectPaths} onBrowse={browse} onConfirm={confirm} onOpenChange={vi.fn()} />
  </I18nProvider>))
  const query = <T extends HTMLElement>(selector: string) => container.querySelector<T>(selector)
  const entry = (name: string) => query<HTMLElement>(`[data-folder-picker-entry$="\\\\${name}"]`)
  const entries = () => Array.from(container.querySelectorAll<HTMLElement>("[data-folder-picker-entry]")).map((row) => row.textContent)
  const crumbPaths = () => Array.from(container.querySelectorAll<HTMLElement>("[data-folder-picker-crumb]")).map((crumb) => crumb.dataset.folderPickerCrumb)
  const selection = () => query<HTMLElement>("[data-folder-picker-selection]")!.dataset.folderPickerSelection
  const key = async (target: HTMLElement, value: string) => act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true }))
  })
  const cleanup = async () => { await act(async () => root.unmount()); container.remove() }
  return { browse, confirm, query, entry, entries, crumbPaths, selection, key, cleanup }
}

describe("folder picker paths", () => {
  // Verifies every crumb opens an absolute path in the path's own separator style.
  // 验证每个路径段都以路径自身的分隔符风格打开绝对路径。
  it.each<[string, string[]]>([
    ["C:\\Users\\dev", ["C:\\", "C:\\Users", "C:\\Users\\dev"]],
    ["C:\\", ["C:\\"]],
    ["/home/dev/src", ["/", "/home", "/home/dev", "/home/dev/src"]],
    ["\\\\server\\share\\team", ["\\\\server\\share", "\\\\server\\share\\team"]],
  ])("splits %s into openable ancestors", (path, expected) => {
    expect(projectFolderPickerCrumbs(path).map((crumb) => crumb.path)).toEqual(expected)
  })

  // Verifies quick places are the distinct project parents, the most used first, without drive roots.
  // 验证常用位置为去重后的项目上级目录，使用最多者在前，且不含盘符根目录。
  it("offers project parent folders as places, most used first", () => {
    expect(projectFolderPickerPlaces(["D:\\work\\one", "C:\\src\\a", "c:\\SRC\\b", "E:\\solo", "/"]).map((place) => place.path))
      .toEqual(["C:\\src", "D:\\work"])
  })

  // Verifies places sharing a name are told apart by their parent's name.
  // 验证同名常用位置通过其上级名称区分。
  it("tells same-named places apart", () => {
    const labels = projectFolderPickerPlaces(["C:\\a\\projects\\x", "D:\\projects\\y", "C:\\c\\src\\z"]).map((place) => place.label)
    expect(labels[2]).toBe("src")
    expect(labels[0]).not.toBe(labels[1])
    expect(labels[0]).toContain("a")
    expect(labels[1]).toContain("D:")
  })
})

describe("folder picker browsing", () => {
  // Verifies one click selects a child for confirmation while the browsed folder stays put.
  // 验证单击选中子目录用于确认，而当前浏览目录保持不变。
  it("selects a child on click and confirms it without entering", async () => {
    const picker = await openPicker()
    try {
      expect(picker.selection()).toBe("C:\\Users\\dev\\projects")
      await act(async () => picker.entry("alpha")!.click())
      expect(picker.selection()).toBe("C:\\Users\\dev\\projects\\alpha")
      expect(picker.browse).toHaveBeenCalledTimes(1)
      await act(async () => picker.query<HTMLButtonElement>("[data-folder-picker-confirm]")!.click())
      expect(picker.confirm).toHaveBeenCalledExactlyOnceWith("C:\\Users\\dev\\projects\\alpha")
    } finally { await picker.cleanup() }
  })

  // Verifies double click enters, a crumb jumps to any ancestor and back returns along the history.
  // 验证双击进入、路径段跳到任一祖先，后退沿历史返回。
  it("enters on double click, jumps by crumb and goes back", async () => {
    const picker = await openPicker()
    try {
      await act(async () => picker.entry("beta")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })))
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects\\beta")
      await act(async () => picker.query<HTMLButtonElement>('[data-folder-picker-crumb="C:\\\\Users"]')!.click())
      expect(picker.entries()).toEqual(["dev"])
      await act(async () => picker.query<HTMLButtonElement>("[data-folder-picker-back]")!.click())
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects\\beta")
      await act(async () => picker.query<HTMLButtonElement>("[data-folder-picker-back]")!.click())
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects")
      expect(picker.query<HTMLButtonElement>("[data-folder-picker-back]")!.disabled).toBe(true)
    } finally { await picker.cleanup() }
  })

  // Verifies typing narrows the rows and Enter opens the only match; Backspace on an empty filter goes up.
  // 验证输入会筛选行、Enter 进入唯一匹配项；筛选为空时 Backspace 返回上级。
  it("filters by name, opens the only match with Enter and goes up with Backspace", async () => {
    const picker = await openPicker()
    try {
      const filter = picker.query<HTMLInputElement>("[data-folder-picker-filter]")!
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(filter, "alp")
        filter.dispatchEvent(new Event("input", { bubbles: true }))
      })
      expect(picker.entries()).toEqual(["alpha"])
      await picker.key(filter, "Enter")
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects\\alpha")
      await picker.key(picker.query<HTMLInputElement>("[data-folder-picker-filter]")!, "Backspace")
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects")
    } finally { await picker.cleanup() }
  })

  // Verifies arrow keys move the highlight that confirmation uses.
  // 验证方向键移动的高亮即确认所用的目录。
  it("moves the selection with the arrow keys", async () => {
    const picker = await openPicker()
    try {
      const list = picker.query<HTMLElement>('[role="listbox"]')!
      await picker.key(list, "ArrowDown")
      await picker.key(list, "ArrowDown")
      expect(picker.selection()).toBe("C:\\Users\\dev\\projects\\beta")
      await picker.key(list, "ArrowRight")
      expect(picker.crumbPaths().at(-1)).toBe("C:\\Users\\dev\\projects\\beta")
    } finally { await picker.cleanup() }
  })

  // Verifies dot folders stay hidden until the toggle reveals them.
  // 验证点目录在打开开关前保持隐藏。
  it("hides dot folders until asked", async () => {
    const picker = await openPicker()
    try {
      expect(picker.entries()).toEqual(["alpha", "beta"])
      await act(async () => picker.query<HTMLButtonElement>("[data-folder-picker-hidden-toggle]")!.click())
      expect(picker.entries()).toHaveLength(3)
    } finally { await picker.cleanup() }
  })

  // Verifies the footer button opens the typed path first so confirmation follows the listing.
  // 验证底部按钮会先打开输入的路径，确认动作随后再以目录列表为准。
  it("opens the typed path from the footer button before confirming", async () => {
    const picker = await openPicker()
    try {
      await act(async () => picker.query<HTMLButtonElement>("[data-folder-picker-edit]")!.click())
      const input = picker.query<HTMLInputElement>(".project-folder-picker-path-input")!
      await act(async () => {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "C:\\Users")
        input.dispatchEvent(new Event("input", { bubbles: true }))
      })
      const confirmButton = picker.query<HTMLButtonElement>("[data-folder-picker-confirm]")!
      expect(confirmButton.disabled).toBe(false)
      expect(picker.query("[data-folder-picker-unbrowsed]")).not.toBeNull()
      await act(async () => confirmButton.click())
      expect(picker.browse).toHaveBeenLastCalledWith("C:\\Users")
      expect(picker.selection()).toBe("C:\\Users")
      expect(picker.confirm).not.toHaveBeenCalled()
    } finally { await picker.cleanup() }
  })

  // Verifies existing projects add their folder as a place and mark their own row.
  // 验证已有项目会把其上级目录加入常用位置，并标记自身所在行。
  it("offers project folders as places and marks existing projects", async () => {
    const picker = await openPicker(["C:\\Users\\dev\\projects\\alpha"])
    try {
      expect(picker.query("[data-folder-picker-place]")?.getAttribute("data-folder-picker-place")).toBe("C:\\Users\\dev\\projects")
      expect(picker.entry("alpha")!.querySelector(".project-folder-picker-badge")).not.toBeNull()
      expect(picker.entry("beta")!.querySelector(".project-folder-picker-badge")).toBeNull()
    } finally { await picker.cleanup() }
  })
})
