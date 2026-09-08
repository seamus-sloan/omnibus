import type { APIRequestContext } from "@playwright/test";

/** One stored position, as `GET /api/progress/{uuid}` reports it. */
export interface ProgressRecord {
  format: "epub" | "audio";
  epub_cfi: string | null;
  audio_position_seconds: number | null;
  progress_percent: number | null;
  total_duration_seconds?: number;
  resolved?: {
    spine_index?: number;
    chapter_title?: string;
    chapter_ordinal?: number;
    chapters_total?: number;
    percent_through_chapter?: number;
    percent_through_book?: number;
    confidence: "high" | "low";
  };
}

/** Every position the signed-in reader holds in one book. */
export interface BookProgress {
  book_uuid: string;
  records: ProgressRecord[];
  furthest: "epub" | "audio" | null;
  linked?: boolean;
}

/**
 * The reader's stored position in one format, or `null` when they have none.
 *
 * The endpoint answers with an envelope over every format — `?format=` narrows
 * `records` rather than changing the shape — so a spec that wants one side
 * still has to reach through it.
 */
export async function storedProgress(
  request: APIRequestContext,
  uuid: string,
  format: "epub" | "audio" = "epub",
): Promise<ProgressRecord | null> {
  const res = await request.get(`/api/progress/${uuid}?format=${format}`);
  if (!res.ok()) return null;
  const body = (await res.json()) as BookProgress | null;
  return body?.records.find((r) => r.format === format) ?? null;
}
