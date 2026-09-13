import type { VisualSourceChange } from "../editor/latex-roundtrip";
import type { SetwrightBridge } from "./bridge";
import type { ProjectSnapshot, SaveState } from "./contracts";
import { createDeclaredSourceEdits, createMinimalSourceEdit } from "./source-edits";

export interface FileDraft {
  text: string;
  sequence: number;
}

export interface WorkspaceSessionState {
  project: ProjectSnapshot;
  drafts: Readonly<Record<string, FileDraft>>;
  paused: boolean;
  error: string | null;
  saving: boolean;
  restoring: boolean;
  closing: boolean;
  pendingConversions: number;
  epoch: number;
}

/** One revision authority for all files in this window. UI navigation never owns writes. */
export class WorkspaceSession {
  private state: WorkspaceSessionState;
  private listeners = new Set<() => void>();
  private queue: Promise<void> = Promise.resolve();
  private sequence = 0;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(project: ProjectSnapshot, private readonly bridge: SetwrightBridge) {
    this.state = { project, drafts: {}, paused: false, error: null, saving: false, restoring: false, closing: false, pendingConversions: 0, epoch: 0 };
  }

  getSnapshot = (): WorkspaceSessionState => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  get saveState(): SaveState {
    if (this.state.paused) return "conflict";
    if (this.state.saving) return "saving";
    return this.hasChanges ? "dirty" : "saved";
  }

  get hasChanges(): boolean {
    return this.state.project.dirty || Object.keys(this.state.drafts).length > 0 || this.state.pendingConversions > 0 || this.state.closing;
  }

  private update(patch: Partial<WorkspaceSessionState>): void {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }

  private cancelSave(): void {
    if (this.saveTimer !== undefined) clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
  }

  private enqueue<T>(action: () => Promise<T>): Promise<T> {
    const task = this.queue.then(action);
    this.queue = task.then(() => undefined, () => undefined);
    return task;
  }

  private pause(cause: unknown): void {
    this.cancelSave();
    this.update({ paused: true, saving: false, error: cause instanceof Error ? cause.message : "The changes could not be saved safely." });
  }

  private requireWritable(): void {
    if (this.state.paused) throw new Error("Writing is paused. Recover retained drafts and retry saving before continuing.");
  }

  private acknowledge(fileId: string, sequence: number): void {
    if (this.state.drafts[fileId]?.sequence !== sequence) return;
    const drafts = { ...this.state.drafts };
    delete drafts[fileId];
    this.update({ drafts });
  }

  edit(fileId: string, text: string, changes?: readonly VisualSourceChange[], basis?: string): void {
    if (this.state.restoring || this.state.closing) return;
    const sequence = ++this.sequence;
    this.cancelSave();
    this.update({ drafts: { ...this.state.drafts, [fileId]: { text, sequence } } });
    if (this.state.paused) return;
    void this.enqueue(async () => {
      if (this.state.paused) return;
      const project = this.state.project;
      const file = project.files.find((candidate) => candidate.id === fileId);
      if (file === undefined || file.kind === "asset" || file.content === null || file.encoding !== "utf8") {
        throw new Error("This file is unavailable for text editing.");
      }
      const declared = changes !== undefined && basis === file.content
        ? await createDeclaredSourceEdits(fileId, file.content, text, changes)
        : null;
      const fallback = declared === null ? await createMinimalSourceEdit(fileId, file.content, text) : null;
      const edits = declared ?? (fallback === null ? [] : [fallback]);
      if (edits.length > 0) {
        const result = await this.bridge.applySourceEdits(project.sessionId, project.revision, edits);
        this.update({ project: { ...project, revision: result.revision, files: result.files, dirty: true } });
      }
      this.acknowledge(fileId, sequence);
      this.scheduleSave();
    }).catch((cause: unknown) => this.pause(cause));
  }

  private scheduleSave(): void {
    this.cancelSave();
    if (!this.state.project.dirty || this.state.paused) return;
    this.saveTimer = setTimeout(() => {
      this.saveTimer = undefined;
      void this.save().catch(() => { /* save records the failure and retains drafts */ });
    }, 750);
  }

  private async saveAccepted(): Promise<void> {
    this.requireWritable();
    const project = this.state.project;
    if (!project.dirty) return;
    this.update({ saving: true });
    try {
      await this.bridge.saveProject(project.sessionId, project.revision);
      this.update({
        project: { ...project, dirty: false, files: project.files.map((file) => ({ ...file, dirty: false })) },
        saving: false,
      });
    } catch (cause) {
      this.pause(cause);
      throw cause;
    }
  }

  save(): Promise<void> {
    this.cancelSave();
    return this.enqueue(() => this.saveAccepted());
  }

  async retrySave(): Promise<void> {
    if (Object.keys(this.state.drafts).length > 0) {
      throw new Error("Copy or discard each retained draft before retrying the accepted source.");
    }
    this.cancelSave();
    await this.enqueue(async () => {
      // A failed response can still have applied in Rust. Reconcile the accepted
      // buffers before admitting more writes, rather than trusting our last ACK.
      const project = await this.bridge.readProject(this.state.project.sessionId);
      this.update({ project, epoch: this.state.epoch + 1 });
      if (Object.keys(this.state.drafts).length > 0) {
        throw new Error("New drafts were entered during recovery. Copy or discard them before retrying.");
      }
      this.update({ paused: false, error: null });
      await this.saveAccepted();
    }).catch((cause: unknown) => { this.pause(cause); throw cause; });
  }

  discardDraft(fileId: string): void {
    if (!this.state.paused) return;
    const drafts = { ...this.state.drafts };
    delete drafts[fileId];
    this.update({ drafts });
  }

  createSnapshot(name: string) {
    this.cancelSave();
    return this.enqueue(async () => {
      this.requireWritable();
      if (name.trim() === "") throw new Error("Enter a version name.");
      await this.saveAccepted();
      return this.bridge.createSnapshot(this.state.project.sessionId, name.trim());
    });
  }

  close(): Promise<void> {
    if (this.state.paused || this.state.restoring || this.state.closing) return Promise.reject(new Error("Recover all changes before closing this paper."));
    this.cancelSave();
    this.update({ closing: true });
    return this.enqueue(async () => {
      await this.saveAccepted();
      await this.bridge.closeProject(this.state.project.sessionId);
    }).catch((cause: unknown) => { this.pause(cause); throw cause; })
      .finally(() => this.update({ closing: false }));
  }

  compile() {
    return this.enqueue(async () => {
      this.requireWritable();
      const project = this.state.project;
      return this.bridge.startCompile(project.sessionId, project.revision, project.settings.engine);
    });
  }

  convert(fileId: string, reviewedText: string, originalSha256: string): Promise<void> {
    this.cancelSave();
    // Protect approved text immediately, including time waiting behind another
    // operation. A clean Rust snapshot cannot represent this submission yet.
    this.update({ pendingConversions: this.state.pendingConversions + 1 });
    return this.enqueue(async () => {
      this.requireWritable();
      const project = this.state.project;
      const file = project.files.find((candidate) => candidate.id === fileId);
      if (file === undefined || file.kind === "asset" || file.encoding !== "nonUtf8" || file.content !== null) {
        throw new Error("This file is unavailable for encoding conversion.");
      }
      const result = await this.bridge.convertFileToUtf8(project.sessionId, project.revision, fileId, reviewedText, originalSha256);
      this.update({ project: { ...project, revision: result.revision, files: result.files, dirty: true } });
      this.scheduleSave();
    }).catch((cause: unknown) => { this.pause(cause); throw cause; })
      .finally(() => this.update({ pendingConversions: this.state.pendingConversions - 1 }));
  }

  restore(snapshotId: string): Promise<ProjectSnapshot> {
    if (this.hasChanges || this.state.paused || this.state.restoring) {
      return Promise.reject(new Error("Save or recover all current changes before restoring a version."));
    }
    this.cancelSave();
    this.update({ restoring: true });
    return this.enqueue(async () => {
      if (this.hasChanges || this.state.paused) throw new Error("The paper changed before the restore could start. Save it first.");
      const project = await this.bridge.restoreSnapshot(this.state.project.sessionId, snapshotId);
      this.update({ project, drafts: {}, error: null, epoch: this.state.epoch + 1 });
      return project;
    }).catch((cause: unknown) => {
      // Restoration may have succeeded even if its IPC response was lost.
      // Reconcile accepted source before applying edits against the old view.
      this.pause(cause);
      throw cause;
    }).finally(() => this.update({ restoring: false }));
  }

  /** For teardown and tests; already accepted writes remain ordered. */
  stopAutosave(): void { this.cancelSave(); }
  settled(): Promise<void> { return this.queue; }
}
