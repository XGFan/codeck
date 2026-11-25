/** One highlighted token: content plus its light / dark theme colors. */
export interface HlToken {
  c: string;
  l?: string;
  d?: string;
}

export interface HlRequest {
  id: number;
  code: string;
  lang: string;
}

export interface HlResponse {
  id: number;
  lines: HlToken[][] | null;
}
