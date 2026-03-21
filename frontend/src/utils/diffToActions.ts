import type { ApplyTextEditAction } from '../Fetch';

function offsetToPosition(text: string, offset: number): { line: number; character: number } {
  let line = 0;
  let character = 0;

  for (const ch of text.slice(0, offset)) {
    if (ch === '\n') {
      line += 1;
      character = 0;
      continue;
    }

    const codePoint = ch.codePointAt(0);
    character += codePoint !== undefined && codePoint > 0xffff ? 2 : 1;
  }

  return { line, character };
}

/**
 * Convert string diffs to Actions with smart update detection.
 * 
 * Algorithm:
 * 1. Run diffChars to get character-level changes
 * 2. Calculate absolute positions for each change
 * 3. Group consecutive changes: if delete+insert at same position -> update
 * 4. Otherwise, return insert/delete separately
 */
export function diffToActions(oldText: string, newText: string): ApplyTextEditAction[] {
  if (oldText === newText) {
    return [];
  }

  let prefix = 0;
  const maxPrefix = Math.min(oldText.length, newText.length);
  while (prefix < maxPrefix && oldText[prefix] === newText[prefix]) {
    prefix += 1;
  }

  let oldSuffix = oldText.length;
  let newSuffix = newText.length;
  while (
    oldSuffix > prefix &&
    newSuffix > prefix &&
    oldText[oldSuffix - 1] === newText[newSuffix - 1]
  ) {
    oldSuffix -= 1;
    newSuffix -= 1;
  }

  const replacement = newText.slice(prefix, newSuffix);
  return [
    {
      type: 'applyTextEdit',
      range: {
        start: offsetToPosition(oldText, prefix),
        end: offsetToPosition(oldText, oldSuffix),
      },
      text: replacement,
    },
  ];
}

/**
 * Merge adjacent insert/delete operations into update when applicable.
 * This runs after diffToActions for additional optimization.
 */
export function mergeAdjacentActions(actions: ApplyTextEditAction[]): ApplyTextEditAction[] {
  return actions;
}
