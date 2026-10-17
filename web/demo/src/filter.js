/** Selecting tasks out of a list. */

/**
 * Tasks due on or before `date`.
 *
 * Dates are ISO day strings (`2026-03-14`), so they compare correctly as text.
 */
export function dueBy(tasks, date) {
  return tasks.filter((task) => task.due && task.due <= date);
}

/** Tasks that are still open, newest first. */
export function open(tasks) {
  return tasks.filter((task) => !task.done).sort((a, b) => b.added.localeCompare(a.added));
}

/** Tasks carrying every one of `tags`. */
export function tagged(tasks, tags) {
  return tasks.filter((task) => tags.every((tag) => task.tags.includes(tag)));
}
