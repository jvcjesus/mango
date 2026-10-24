export function hash(text: string): number {
  let h = 0;
  for (let i = 0; i < text.length; i++) h = (h * 31 + text.charCodeAt(i)) >>> 0;
  return h;
}

/** "3 h ago", from a time in seconds. */
export function ago(seconds: number): string {
  const diff = Math.max(0, Date.now() / 1000 - seconds);
  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} h ago`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} d ago`;
  return new Date(seconds * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** A heading for a day: Today, Yesterday, or the date. From a time in seconds. */
export function dayLabel(seconds: number): string {
  const day = new Date(seconds * 1000);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((start(new Date()) - start(day)) / 86_400_000);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  return day.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: days > 300 ? 'numeric' : undefined });
}

/** Seconds since the epoch at the start of that day, to group by. */
export function dayKey(seconds: number): number {
  const d = new Date(seconds * 1000);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() / 1000;
}

export const STATUS = ['Unknown', 'Ongoing', 'Completed', 'Licensed', 'Finished', 'Cancelled', 'On hiatus'];

const names = typeof Intl.DisplayNames === 'function' ? new Intl.DisplayNames(undefined, { type: 'language' }) : null;

export function langName(code: string): string {
  if (code === 'all') return 'Multi';
  if (code === 'other') return 'Other';
  try {
    return names?.of(code) ?? code;
  } catch {
    return code;
  }
}

/** "Ch. 12.5" when the number is known, else the chapter's own name. */
export function chapterShort(chapter: { number: number; name: string }): string {
  return chapter.number >= 0 ? `Ch. ${+chapter.number.toFixed(2)}` : chapter.name;
}

export type ReleaseType = 'chapter' | 'volume';

export type ReleaseInfo = {
  releaseType: ReleaseType;
  volume: number | null;
  chapterNumber: number | null;
};

/** Parses volume number from a chapter name like "Vol. 1 Ch. 5" or "Volume 2 - 14", if present. */
export function parseVolume(name: string): number | null {
  if (!name) return null;
  const lower = name.toLowerCase();

  // 1. Check for explicit "vol." / "volume" / "vol " / "vol_"
  const volMatch = lower.match(/(?:^|[^\w])vol(?:ume)?\.?\s*(\d+(?:\.\d+)?)/i);
  if (volMatch && volMatch[1]) {
    const val = parseFloat(volMatch[1]);
    return isNaN(val) ? null : val;
  }

  // 2. Check for short "v<number>" pattern (e.g. "v3 c20", "v01")
  // Must avoid matching revision/version notations such as "Chapter 5 v2" or "Ch. 12 (v2)"
  const vMatches = lower.matchAll(/(?:^|[\s_([{<.,-])v\.?\s*(\d+(?:\.\d+)?)(?=[^\w]|$)/gi);
  for (const m of vMatches) {
    const matchIndex = m.index ?? 0;
    const beforeText = lower.slice(0, matchIndex);
    const hasPriorChapter = /(?:^|[^\w])(?:chapter|ch\.|ch[\s\-_]|episode|ep\.|ep[\s\-_]|#)/i.test(beforeText);
    if (hasPriorChapter) {
      continue; // Skip revision/version strings like "Chapter 5 v2"
    }
    const valStr = m[1];
    const rest = lower.slice(matchIndex + m[0].length);
    if (rest.startsWith('.')) continue;

    const val = parseFloat(valStr);
    if (isNaN(val)) continue;

    const after = lower.slice(matchIndex + m[0].length).trimStart();
    const followedByChapter = after.startsWith('c') || after.startsWith('ch') || after.startsWith('#') || after.startsWith('-');
    const inBrackets = matchIndex > 0 && ['[', '(', '{', '<'].includes(lower[matchIndex] === 'v' ? lower[matchIndex - 1] : lower[matchIndex]);
    const atStart = matchIndex === 0 || /^[\s_([{<.,-]+$/.test(beforeText);

    if (atStart || inBrackets || followedByChapter) {
      return val;
    }
  }

  return null;
}

/** Parses release semantics distinguishing pure Volume releases from Chapter releases. */
export function parseRelease(name: string, knownNumber = -1): ReleaseInfo {
  const volume = parseVolume(name);
  if (!name) {
    return { releaseType: 'chapter', volume: null, chapterNumber: knownNumber >= 0 ? knownNumber : null };
  }
  const lower = name.toLowerCase();
  const hasChapterWord = /(?:^|[^\w])(?:chapter|ch\.|ch[\s\-_]|episode|ep\.|ep[\s\-_]|act|#)/i.test(lower);
  const hasCChapter = volume !== null && /\bv\d+\s*[-_ ]*\s*c\d+/i.test(lower);

  // Check if there is an explicit chapter number
  let chNum: number | null = knownNumber >= 0 ? knownNumber : null;
  const numMatch = lower.match(/(?:^|[^\w])(?:chapter|ch\.|ch[\s\-_]|episode|ep\.|ep[\s\-_]|#)\s*(\d+(?:\.\d+)?)/i);
  if (numMatch && numMatch[1]) {
    const parsed = parseFloat(numMatch[1]);
    if (!isNaN(parsed)) chNum = parsed;
  }

  const isVolume = volume !== null && !hasChapterWord && !hasCChapter && (chNum === null || chNum === volume);

  if (isVolume) {
    return {
      releaseType: 'volume',
      volume,
      chapterNumber: null,
    };
  }

  return {
    releaseType: 'chapter',
    volume,
    chapterNumber: chNum,
  };
}

/** Formats a volume number for display (e.g. "Volume 1", "Vol. 2"). */
export function volumeHeading(vol: number | null): string {
  return vol !== null ? `VOLUME ${vol}` : 'OTHER CHAPTERS';
}

/** Runs `run` each time the element comes near the viewport. For lists that load as they are scrolled. */
export function whenVisible(node: HTMLElement, run: () => void) {
  let current = run;
  const observer = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && current(), { rootMargin: '800px' });
  observer.observe(node);
  return {
    update(next: () => void) {
      current = next;
    },
    destroy() {
      observer.disconnect();
    },
  };
}
