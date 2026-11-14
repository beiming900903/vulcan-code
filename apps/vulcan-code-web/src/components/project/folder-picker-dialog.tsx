import React from "react";
import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Check,
  ChevronRight,
  Eye,
  Folder,
  FolderHeart,
  HardDrive,
  PencilLine,
  RefreshCw,
  Search,
} from "lucide-react";
import { Button } from "@/ui-kit/primitives/button";
import { InlineStreamPlaceholder } from "@/ui-kit/primitives/stream-placeholder";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/ui-kit/primitives/dialog";
import { Input } from "@/ui-kit/primitives/input";
import { Spinner } from "@/ui-kit/primitives/spinner";
import { formatMessage, useI18n } from "@/i18n";
import type { ProjectDirectoryListing } from "@/types";
import { projectPathKey } from "./project-path-key";
import "@/ui-kit/styles/features/project-folder-picker.css";
import "@/ui-kit/styles/features/settings-forms.css"

// PLACE_LIMIT caps how many project parent folders the sidebar offers as quick places.
// PLACE_LIMIT 限制侧栏作为常用位置提供的项目上级目录数量。
const PLACE_LIMIT = 6;

// ProjectFolderPickerCrumb is one clickable ancestor of the browsed directory.
// ProjectFolderPickerCrumb 是当前浏览目录的一个可点击祖先。
export interface ProjectFolderPickerCrumb {
  // Segment shown in the breadcrumb.
  // 面包屑中显示的路径段。
  label: string;
  // Absolute path the segment opens.
  // 该路径段打开的绝对路径。
  path: string;
}

// projectFolderPickerCrumbs splits one absolute path into its root and every ancestor, keeping the
// path's own separator so each crumb opens a path the service accepts.
// projectFolderPickerCrumbs 将绝对路径拆为根与各级祖先，并沿用路径自身的分隔符，使每一段都能打开服务接受的路径。
export function projectFolderPickerCrumbs(path: string): ProjectFolderPickerCrumb[] {
  const windows = /^[a-zA-Z]:/.test(path) || path.startsWith("\\\\");
  const parts = path.split(/[\\/]+/).filter((part) => part.length > 0);
  if (!windows) {
    const crumbs: ProjectFolderPickerCrumb[] = [{ label: "/", path: "/" }];
    let current = "";
    for (const part of parts) {
      current += `/${part}`;
      crumbs.push({ label: part, path: current });
    }
    return crumbs;
  }
  const unc = path.startsWith("\\\\");
  const rootParts = unc ? parts.slice(0, 2) : parts.slice(0, 1);
  if (rootParts.length === 0) return [];
  const rootLabel = unc ? `\\\\${rootParts.join("\\")}` : rootParts[0];
  const rootPath = unc ? rootLabel : `${rootLabel}\\`;
  const crumbs: ProjectFolderPickerCrumb[] = [{ label: rootLabel, path: rootPath }];
  let current = unc ? rootPath : rootLabel;
  for (const part of parts.slice(rootParts.length)) {
    current += `\\${part}`;
    crumbs.push({ label: part, path: current });
  }
  return crumbs;
}

// projectFolderPickerPlaces returns the folders that hold existing projects, most used first, so
// a new project next to the others is one click away. Drive roots are left to the drive list, and
// folders sharing a name carry their own parent's name to tell them apart.
// projectFolderPickerPlaces 返回已有项目所在的上级目录（使用最多者在前），使与其他项目同级的新项目一键可达。
// 盘符根目录交给磁盘列表；同名目录附带其上级名称以便区分。
export function projectFolderPickerPlaces(projectPaths: readonly string[]): ProjectFolderPickerCrumb[] {
  const places = new Map<string, { place: ProjectFolderPickerCrumb; owner: string; count: number; order: number }>();
  for (const projectPath of projectPaths) {
    const crumbs = projectFolderPickerCrumbs(projectPath);
    if (crumbs.length < 3) continue;
    const parent = crumbs[crumbs.length - 2];
    const owner = crumbs[crumbs.length - 3].label;
    const key = projectPathKey(parent.path);
    const known = places.get(key);
    if (known) known.count += 1;
    else places.set(key, { place: parent, owner, count: 1, order: places.size });
  }
  const chosen = [...places.values()]
    .sort((left, right) => right.count - left.count || left.order - right.order)
    .slice(0, PLACE_LIMIT);
  const labelCounts = new Map<string, number>();
  for (const entry of chosen) labelCounts.set(entry.place.label, (labelCounts.get(entry.place.label) ?? 0) + 1);
  return chosen.map(({ place, owner }) => ((labelCounts.get(place.label) ?? 0) > 1
    ? { label: `${owner} › ${place.label}`, path: place.path }
    : place));
}

// FolderPickerSidebar lists the quick places above the drives; the place or drive owning the browsed
// directory is highlighted.
// FolderPickerSidebar 在磁盘上方列出常用位置；拥有当前浏览目录的位置或磁盘会高亮。
export function FolderPickerSidebar(props: {
  places: readonly ProjectFolderPickerCrumb[];
  roots: readonly ProjectFolderPickerCrumb[];
  currentKey: string | null;
  activeRootPath: string | null;
  loaded: boolean;
  busy: boolean;
  onOpen: (path: string) => void;
}) {
  const { t } = useI18n();
  return (
    <aside aria-label={t("projectFolderPicker.rootsAria")} className="project-folder-picker-sidebar">
      {props.places.length > 0 ? (
        <section className="project-folder-picker-group">
          <h3>{t("projectFolderPicker.places")}</h3>
          {props.places.map((place) => (
            <button
              className="project-folder-picker-place"
              data-active={projectPathKey(place.path) === props.currentKey ? "" : undefined}
              data-folder-picker-place={place.path}
              disabled={props.busy}
              key={place.path}
              onClick={() => props.onOpen(place.path)}
              title={place.path}
              type="button"
            >
              <FolderHeart size={15} />
              <span>{place.label}</span>
            </button>
          ))}
        </section>
      ) : null}
      <section className="project-folder-picker-group">
        <h3>{t("projectFolderPicker.drives")}</h3>
        {props.roots.map((root) => (
          <button
            className="project-folder-picker-place"
            data-active={root.path === props.activeRootPath ? "" : undefined}
            disabled={props.busy}
            key={root.path}
            onClick={() => props.onOpen(root.path)}
            type="button"
          >
            <HardDrive size={15} />
            <span>{root.label}</span>
          </button>
        ))}
        {props.loaded && props.roots.length === 0 ? (
          <div className="project-folder-picker-empty">{t("projectFolderPicker.noRoots")}</div>
        ) : null}
      </section>
    </aside>
  );
}

// ProjectFolderPickerDialogProps groups service-backed project directory picker inputs.
// ProjectFolderPickerDialogProps 汇总服务驱动的项目目录选择器输入。
export interface ProjectFolderPickerDialogProps {
  // Optional dialog title override for host features reusing the directory browser.
  // 宿主功能复用目录浏览器时使用的可选弹窗标题覆盖。
  title?: string;
  // Optional confirmation label override.
  // 可选的确认按钮文案覆盖。
  confirmLabel?: string;
  // Optional native directory opened immediately when the dialog becomes visible.
  // 对话框变为可见时立即打开的可选原生目录。
  initialPath?: string | null;
  // Paths of existing projects: their folders become quick places and their rows are marked.
  // 已有项目的路径：其上级目录成为常用位置，其所在行会被标记。
  projectPaths?: readonly string[];
  // Whether the picker dialog is currently open.
  // 当前选择器弹窗是否打开。
  open: boolean;
  // Handles dialog visibility changes.
  // 处理弹窗可见性变化。
  onOpenChange: (open: boolean) => void;
  // Loads one directory listing from the Core Service.
  // 从 Core Service 加载单个目录列表。
  onBrowse: (path: string | null) => Promise<ProjectDirectoryListing>;
  // Confirms the selected directory path.
  // 确认选中的目录路径。
  onConfirm: (path: string) => Promise<void>;
}

// ProjectFolderPickerDialog renders a service-host directory browser: a clickable breadcrumb, a
// filterable folder list where one click selects and a double click (or Enter) opens, back and up
// navigation, and quick places beside the drives.
// ProjectFolderPickerDialog 渲染服务宿主目录浏览器：可点击的面包屑、可筛选的目录列表（单击选择，双击或 Enter
// 进入）、后退与上级导航，以及磁盘旁的常用位置。
export function ProjectFolderPickerDialog({
  title,
  confirmLabel,
  initialPath,
  projectPaths,
  open,
  onBrowse,
  onConfirm,
  onOpenChange,
}: ProjectFolderPickerDialogProps) {
  const { t } = useI18n();
  // listing stores the current directory listing returned by the service.
  // listing 保存服务返回的当前目录列表。
  const [listing, setListing] = React.useState<ProjectDirectoryListing | null>(null);
  // pathInput stores the editable path field shown while editing the path.
  // pathInput 保存编辑路径时显示的可编辑路径输入框。
  const [pathInput, setPathInput] = React.useState("");
  // editingPath swaps the breadcrumb for the path field.
  // editingPath 将面包屑切换为路径输入框。
  const [editingPath, setEditingPath] = React.useState(false);
  // editingPathRef lets the dialog's Escape handler leave editing instead of closing.
  // editingPathRef 让对话框的 Escape 处理先退出编辑而不是关闭弹窗。
  const editingPathRef = React.useRef(false);
  editingPathRef.current = editingPath;
  // history holds the directories left by navigation, newest last, for the back button.
  // history 保存导航离开的目录（最新在末尾），供后退按钮使用。
  const [history, setHistory] = React.useState<string[]>([]);
  // selectedChild is the highlighted child folder; null selects the browsed directory itself.
  // selectedChild 是高亮的子目录；为 null 时选择当前浏览目录本身。
  const [selectedChild, setSelectedChild] = React.useState<string | null>(null);
  // filter narrows the rows of the browsed directory by name.
  // filter 按名称筛选当前目录的行。
  const [filter, setFilter] = React.useState("");
  // showHidden reveals dot folders, which stay out of the way by default.
  // showHidden 显示以点开头的目录，默认隐藏以免干扰。
  const [showHidden, setShowHidden] = React.useState(false);
  // loading tracks directory listing requests.
  // loading 跟踪目录列表请求。
  const [loading, setLoading] = React.useState(false);
  // submitting tracks the final project-add request.
  // submitting 跟踪最终添加项目请求。
  const [submitting, setSubmitting] = React.useState(false);
  // submittingRef synchronously prevents duplicate confirmation before React rerenders.
  // submittingRef 在 React 重新渲染前同步阻止重复确认。
  const submittingRef = React.useRef(false);
  // error stores the latest picker error.
  // error 保存最近一次选择器错误。
  const [error, setError] = React.useState<string | null>(null);
  // browseSequenceRef rejects stale directory listing responses.
  // browseSequenceRef 丢弃过期目录列表响应。
  const browseSequenceRef = React.useRef(0);
  // crumbBarRef keeps the deepest crumb in view after each navigation.
  // crumbBarRef 在每次导航后让最深的路径段保持可见。
  const crumbBarRef = React.useRef<HTMLDivElement | null>(null);
  // listRef locates the highlighted row so keyboard moves keep it in view.
  // listRef 用于定位高亮行，使键盘移动时保持可见。
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const directoryRoots = React.useMemo(() => listing?.roots ?? [], [listing?.roots]);
  const places = React.useMemo(() => projectFolderPickerPlaces(projectPaths ?? []), [projectPaths]);
  const projectKeys = React.useMemo(() => new Set((projectPaths ?? []).map(projectPathKey)), [projectPaths]);
  const crumbs = React.useMemo(() => (listing ? projectFolderPickerCrumbs(listing.path) : []), [listing]);
  const currentKey = listing ? projectPathKey(listing.path) : null;
  // activeRootPath stores the root currently owning the displayed directory.
  // activeRootPath 保存当前显示目录所属的根目录路径。
  const activeRootPath = React.useMemo(() => {
    if (currentKey === null) return null;
    let matchedPath: string | null = null;
    let matchedLength = -1;
    for (const root of directoryRoots) {
      const rootKey = projectPathKey(root.path);
      const rootMatches = rootKey === "/" ? currentKey.startsWith("/") : currentKey === rootKey || currentKey.startsWith(`${rootKey}/`);
      if (rootMatches && rootKey.length > matchedLength) {
        matchedPath = root.path;
        matchedLength = rootKey.length;
      }
    }
    return matchedPath;
  }, [currentKey, directoryRoots]);
  const hiddenCount = listing?.entries.filter((entry) => entry.hidden).length ?? 0;
  // visibleEntries applies the hidden toggle and the name filter to the browsed directory.
  // visibleEntries 对当前目录应用隐藏开关与名称筛选。
  const visibleEntries = React.useMemo(() => {
    const needle = filter.trim().toLocaleLowerCase();
    return (listing?.entries ?? []).filter((entry) => (showHidden || !entry.hidden)
      && (needle === "" || entry.name.toLocaleLowerCase().includes(needle)));
  }, [filter, listing, showHidden]);

  // loadDirectory requests one directory listing and updates picker state; `from` is the directory
  // being left, recorded for the back button.
  // loadDirectory 请求单个目录列表并更新选择器状态；`from` 是正在离开的目录，记录供后退使用。
  const loadDirectory = React.useCallback(async (path: string | null, from: string | null = null) => {
    const sequence = browseSequenceRef.current + 1;
    browseSequenceRef.current = sequence;
    setLoading(true);
    setError(null);
    try {
      const nextListing = await onBrowse(path);
      if (browseSequenceRef.current !== sequence) {
        return;
      }
      if (from !== null && projectPathKey(from) !== projectPathKey(nextListing.path)) {
        setHistory((previous) => [...previous, from]);
      }
      setListing(nextListing);
      setPathInput(nextListing.path);
      setEditingPath(false);
      setSelectedChild(null);
      setFilter("");
    } catch (nextError) {
      if (browseSequenceRef.current === sequence) {
        setError(nextError instanceof Error ? nextError.message : String(nextError));
      }
    } finally {
      if (browseSequenceRef.current === sequence) {
        setLoading(false);
      }
    }
  }, [onBrowse]);

  React.useEffect(() => {
    if (!open) {
      browseSequenceRef.current += 1;
      setListing(null); setPathInput(""); setLoading(false); setError(null);
      setHistory([]); setEditingPath(false); setSelectedChild(null); setFilter(""); setShowHidden(false);
      return;
    }
    setListing(null);
    void loadDirectory(initialPath ?? null);
    return () => { browseSequenceRef.current += 1; };
  }, [initialPath, loadDirectory, open]);

  React.useEffect(() => {
    const bar = crumbBarRef.current;
    if (bar) bar.scrollLeft = bar.scrollWidth;
  }, [crumbs, editingPath]);

  React.useEffect(() => {
    if (selectedChild === null) return;
    const row = listRef.current?.querySelector<HTMLElement>('[aria-selected="true"]');
    row?.scrollIntoView?.({ block: "nearest" });
  }, [selectedChild]);

  // navigate opens one directory and records the current one for the back button.
  // navigate 打开一个目录，并记录当前目录供后退使用。
  const navigate = (path: string) => void loadDirectory(path, listing?.path ?? null);
  // goBack reopens the directory left most recently without recording a new step.
  // goBack 重新打开最近离开的目录，不记录新的步骤。
  const goBack = () => {
    const previous = history[history.length - 1];
    if (previous === undefined) return;
    setHistory(history.slice(0, -1));
    void loadDirectory(previous);
  };
  const goUp = () => {
    if (listing?.parentPath) navigate(listing.parentPath);
  };

  // selectedPath is authorized only by the listing that owns it: a highlighted child of the browsed
  // directory, or the directory itself while the path field is unchanged.
  // selectedPath 仅由拥有它的目录列表授权：当前目录中高亮的子目录，或路径输入未改写时的当前目录本身。
  const pathEdited = listing !== null && pathInput !== listing.path;
  const selectedPath = !listing?.canSelect || pathEdited
    ? null
    : (selectedChild !== null && visibleEntries.some((entry) => entry.path === selectedChild) ? selectedChild : listing.path);
  const busy = loading || submitting;

  // handleConfirm submits the selected path.
  // handleConfirm 提交选中的路径。
  const handleConfirm = React.useCallback(async () => {
    if (!selectedPath || submittingRef.current) {
      return;
    }
    submittingRef.current = true;
    setSubmitting(true);
    setError(null);
    try {
      await onConfirm(selectedPath);
      onOpenChange(false);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      submittingRef.current = false;
      setSubmitting(false);
    }
  }, [onConfirm, onOpenChange, selectedPath]);

  // openTypedPath loads the manually entered path when it is not empty.
  // openTypedPath 在输入非空时加载手动输入的路径。
  const openTypedPath = () => {
    const typed = pathInput.trim();
    if (typed !== "") navigate(typed);
  };

  // handlePathSubmit loads the manually entered path.
  // handlePathSubmit 加载手动输入的路径。
  const handlePathSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    openTypedPath();
  };

  // cancelPathEdit restores the browsed path and the breadcrumb.
  // cancelPathEdit 恢复当前浏览路径与面包屑。
  const cancelPathEdit = () => {
    setEditingPath(false);
    if (listing) setPathInput(listing.path);
  };

  // moveSelection steps the highlight through the visible rows, starting at either end.
  // moveSelection 在可见行间移动高亮，未选中时从两端开始。
  const moveSelection = (step: 1 | -1) => {
    if (visibleEntries.length === 0) return;
    const index = visibleEntries.findIndex((entry) => entry.path === selectedChild);
    const next = index < 0
      ? (step === 1 ? 0 : visibleEntries.length - 1)
      : Math.min(visibleEntries.length - 1, Math.max(0, index + step));
    setSelectedChild(visibleEntries[next].path);
  };

  // openHighlighted enters the highlighted row, or the only row left by the filter.
  // openHighlighted 进入高亮行，或筛选后仅剩的一行。
  const openHighlighted = () => {
    const target = visibleEntries.find((entry) => entry.path === selectedChild)
      ?? (visibleEntries.length === 1 ? visibleEntries[0] : undefined);
    if (target) navigate(target.path);
  };

  // handleBrowseKeys gives the filter field and the list one keyboard model: arrows move, Enter opens,
  // Backspace on an empty filter goes up.
  // handleBrowseKeys 让筛选框与列表共用一套键盘操作：方向键移动，Enter 进入，筛选为空时 Backspace 返回上级。
  const handleBrowseKeys = (event: React.KeyboardEvent, inField: boolean) => {
    if (busy) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Enter" || (!inField && event.key === "ArrowRight")) {
      event.preventDefault();
      openHighlighted();
    } else if ((event.key === "Backspace" && filter === "") || (!inField && event.key === "ArrowLeft")) {
      event.preventDefault();
      goUp();
    }
  };

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !submittingRef.current && onOpenChange(nextOpen)}>
      <DialogContent
        className="project-folder-picker-dialog"
        size="xl"
        scrollMode="content-scroll"
        onEscapeKeyDown={(event) => {
          if (submittingRef.current) {
            event.preventDefault();
          } else if (editingPathRef.current) {
            event.preventDefault();
            cancelPathEdit();
          }
        }}
        onInteractOutside={(event) => {
          if (submittingRef.current) {
            event.preventDefault();
          }
        }}
        showCloseButton={!submitting}
      >
        <DialogHeader>
          <DialogTitle>{title ?? t("projectFolderPicker.title")}</DialogTitle>
          <DialogDescription>{t("projectFolderPicker.hint")}</DialogDescription>
        </DialogHeader>
        <div className="project-folder-picker-nav">
          <Button aria-label={t("projectFolderPicker.back")} title={t("projectFolderPicker.back")} disabled={history.length === 0 || busy}
            onClick={goBack} size="icon-sm" type="button" variant="ghost" data-folder-picker-back="">
            <ArrowLeft size={15} />
          </Button>
          <Button aria-label={t("projectFolderPicker.parent")} title={t("projectFolderPicker.parent")} disabled={!listing?.parentPath || busy}
            onClick={goUp} size="icon-sm" type="button" variant="ghost" data-folder-picker-up="">
            <ArrowUp size={15} />
          </Button>
          {editingPath ? (
            <form className="project-folder-picker-path-form" onSubmit={handlePathSubmit}>
              <Input
                aria-label={t("projectFolderPicker.pathAria")}
                autoFocus
                className="project-folder-picker-path-input"
                disabled={submitting}
                onChange={(event) => setPathInput(event.target.value)}
                onFocus={(event) => event.currentTarget.select()}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    cancelPathEdit();
                  }
                }}
                value={pathInput}
              />
              <Button aria-label={t("projectFolderPicker.openPath")} title={t("projectFolderPicker.openPath")} disabled={busy}
                size="icon-sm" type="submit" variant="ghost">
                {loading ? <Spinner /> : <ArrowRight size={15} />}
              </Button>
            </form>
          ) : (
            <div className="project-folder-picker-crumbs">
              <nav aria-label={t("projectFolderPicker.pathAria")} className="project-folder-picker-crumb-track" ref={crumbBarRef}>
                {crumbs.map((crumb, index) => {
                  const last = index === crumbs.length - 1;
                  return (
                    <React.Fragment key={crumb.path}>
                      {index > 0 ? <ChevronRight aria-hidden="true" className="project-folder-picker-crumb-sep" size={13} /> : null}
                      <button
                        aria-current={last ? "location" : undefined}
                        className="project-folder-picker-crumb"
                        data-folder-picker-crumb={crumb.path}
                        disabled={busy || last}
                        onClick={() => navigate(crumb.path)}
                        type="button"
                      >
                        {crumb.label}
                      </button>
                    </React.Fragment>
                  );
                })}
                {listing ? null : <InlineStreamPlaceholder label={t("workspaceFiles.loading")} />}
              </nav>
              <button aria-hidden="true" className="project-folder-picker-crumb-fill" disabled={!listing || submitting}
                onClick={() => setEditingPath(true)} tabIndex={-1} type="button" />
              <Button aria-label={t("projectFolderPicker.editPath")} title={t("projectFolderPicker.editPath")} disabled={!listing || submitting}
                onClick={() => setEditingPath(true)} size="icon-sm" type="button" variant="ghost" data-folder-picker-edit="">
                <PencilLine size={14} />
              </Button>
            </div>
          )}
          <Button aria-label={t("skills.action.refresh")} title={t("skills.action.refresh")} disabled={!listing || busy}
            onClick={() => void loadDirectory(listing?.path ?? null)} size="icon-sm" type="button" variant="ghost">
            {loading ? <Spinner /> : <RefreshCw size={14} />}
          </Button>
        </div>
        <div className="project-folder-picker-browser">
          <FolderPickerSidebar
            activeRootPath={activeRootPath}
            busy={busy}
            currentKey={currentKey}
            loaded={listing !== null}
            onOpen={navigate}
            places={places}
            roots={directoryRoots}
          />
          <section className="project-folder-picker-directory">
            <div className="project-folder-picker-filter">
              <Search aria-hidden="true" size={14} />
              <input
                aria-label={t("projectFolderPicker.filter")}
                autoFocus
                data-folder-picker-filter=""
                disabled={submitting}
                onChange={(event) => { setFilter(event.target.value); setSelectedChild(null); }}
                onKeyDown={(event) => handleBrowseKeys(event, true)}
                placeholder={t("projectFolderPicker.filter")}
                value={filter}
              />
              {hiddenCount > 0 ? (
                <button
                  aria-pressed={showHidden}
                  className="project-folder-picker-toggle"
                  data-folder-picker-hidden-toggle=""
                  onClick={() => setShowHidden((value) => !value)}
                  type="button"
                >
                  <Eye size={13} />
                  {formatMessage(t("projectFolderPicker.showHidden"), { count: hiddenCount })}
                </button>
              ) : null}
            </div>
            <div
              aria-activedescendant={selectedChild !== null ? `folder-row-${visibleEntries.findIndex((entry) => entry.path === selectedChild)}` : undefined}
              aria-busy={loading}
              aria-label={listing?.path}
              className="project-folder-picker-list"
              onKeyDown={(event) => handleBrowseKeys(event, false)}
              ref={listRef}
              role="listbox"
              tabIndex={0}
            >
              {visibleEntries.map((entry, index) => {
                const selected = entry.path === selectedChild;
                const isProject = projectKeys.has(projectPathKey(entry.path));
                return (
                  <div
                    aria-selected={selected}
                    className="project-folder-picker-row"
                    data-folder-picker-entry={entry.path}
                    id={`folder-row-${index}`}
                    key={entry.path}
                    onClick={() => !busy && setSelectedChild(selected ? null : entry.path)}
                    onDoubleClick={() => !busy && navigate(entry.path)}
                    role="option"
                  >
                    <Folder size={15} />
                    <span className="project-folder-picker-row-name">{entry.name}</span>
                    {isProject ? <small className="project-folder-picker-badge">{t("projectFolderPicker.isProject")}</small> : null}
                    {entry.hidden ? <small>{t("projectFolderPicker.hidden")}</small> : null}
                    <button
                      aria-label={t("projectFolderPicker.enter")}
                      className="project-folder-picker-enter"
                      data-folder-picker-enter={entry.path}
                      disabled={busy}
                      onClick={(event) => { event.stopPropagation(); navigate(entry.path); }}
                      tabIndex={-1}
                      title={t("projectFolderPicker.enter")}
                      type="button"
                    >
                      <ChevronRight size={15} />
                    </button>
                  </div>
                );
              })}
              {listing && visibleEntries.length === 0 ? (
                <div className="project-folder-picker-empty">
                  {t(listing.entries.length === 0 || (filter.trim() === "" && !showHidden) ? "projectFolderPicker.noChildren" : "projectFolderPicker.noMatches")}
                </div>
              ) : null}
            </div>
          </section>
        </div>
        {pathEdited ? <p role="status" className="vk-form-text" data-folder-picker-unbrowsed="">{t("project.folderPicker.openTheEnteredPath")}</p> : null}
        {error ? <div role="alert" className="project-folder-picker-error">{error}</div> : null}
        {listing?.notice ? <div className="project-folder-picker-notice">{listing.notice}</div> : null}
        <DialogFooter className="project-folder-picker-footer">
          <div className="project-folder-picker-selection" data-folder-picker-selection={selectedPath ?? ""}>
            {selectedPath ? <>
              <span>{t("projectFolderPicker.selected")}</span>
              <strong title={selectedPath}>{selectedPath}</strong>
            </> : null}
          </div>
          <Button disabled={submitting} onClick={() => onOpenChange(false)} type="button" variant="outline">
            {t("conversation.action.cancel")}
          </Button>
          {/* While the path field is edited the primary button opens that path first; the
              selection is only confirmed once the listing owns it again.
              路径输入被编辑时主按钮先打开该路径；目录列表重新拥有选择后才可确认。 */}
          <Button disabled={pathEdited ? busy || pathInput.trim() === "" : !selectedPath || busy}
            onClick={pathEdited ? openTypedPath : handleConfirm} type="button" data-folder-picker-confirm="">
            {submitting ? <Spinner /> : (pathEdited ? <ArrowRight size={16} /> : <Check size={16} />)}
            {submitting ? t("projectFolderPicker.adding") : (pathEdited ? t("projectFolderPicker.openPath") : (confirmLabel ?? t("sidebar.addProject")))}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
