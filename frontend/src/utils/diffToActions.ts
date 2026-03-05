import { diffChars } from 'diff';
import type { Action, InsertAction, DeleteAction, UpdateAction } from '../Fetch';

/**
 * Convert string diffs to Actions with smart update detection.
 * 
 * Algorithm:
 * 1. Run diffChars to get character-level changes
 * 2. Calculate absolute positions for each change
 * 3. Group consecutive changes: if delete+insert at same position -> update
 * 4. Otherwise, return insert/delete separately
 */
export function diffToActions(oldText: string, newText: string): Action[] {
  const diffs = diffChars(oldText, newText);
  const actions: Action[] = [];

  let oldPos = 0;
  let newPos = 0;
  let i = 0;

  while (i < diffs.length) {
    const current = diffs[i];

    if (current.added) {
      // Check if next is a deletion at similar position (update case)
      if (
        i + 1 < diffs.length &&
        diffs[i + 1].removed &&
        newPos === oldPos
      ) {
        // This is an update: delete old text, insert new text
        const deleteAction: UpdateAction = {
          type: 'update',
          start: oldPos,
          end: oldPos + (diffs[i + 1].value?.length || 0),
          text: current.value,
        };
        actions.push(deleteAction);
        oldPos += diffs[i + 1].value?.length || 0;
        newPos += current.value.length;
        i += 2;
      } else {
        // Plain insertion
        const insertAction: InsertAction = {
          type: 'insert',
          offset: newPos,
          text: current.value,
        };
        actions.push(insertAction);
        newPos += current.value.length;
        i++;
      }
    } else if (current.removed) {
      // Plain deletion (not preceded by addition)
      const deleteAction: DeleteAction = {
        type: 'delete',
        start: oldPos,
        end: oldPos + current.value.length,
      };
      actions.push(deleteAction);
      oldPos += current.value.length;
      i++;
    } else {
      // Equal text, skip
      oldPos += current.value.length;
      newPos += current.value.length;
      i++;
    }
  }

  return actions;
}

/**
 * Merge adjacent insert/delete operations into update when applicable.
 * This runs after diffToActions for additional optimization.
 */
export function mergeAdjacentActions(actions: Action[]): Action[] {
  const merged: Action[] = [];

  for (let i = 0; i < actions.length; i++) {
    const current = actions[i];
    const next = actions[i + 1];

    // Check if current is delete and next is insert at the same position
    if (
      current.type === 'delete' &&
      next &&
      next.type === 'insert' &&
      (current as DeleteAction).end === (next as InsertAction).offset
    ) {
      const deleteAction = current as DeleteAction;
      const insertAction = next as InsertAction;

      // Merge into update
      const updateAction: UpdateAction = {
        type: 'update',
        start: deleteAction.start,
        end: deleteAction.end,
        text: insertAction.text,
      };
      merged.push(updateAction);
      i++; // Skip next action since we merged it
    } else {
      merged.push(current);
    }
  }

  return merged;
}
