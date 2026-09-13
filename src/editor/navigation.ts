/** A source position in JavaScript UTF-16 offsets, scoped to one file. */
export interface SourceNavigation {
  fileId: string;
  sourceOffset: number;
  requestId: number;
}
