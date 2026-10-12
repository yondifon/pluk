/** The line saying who a post goes out as. A LinkedIn page can arrive as its
 * number alone, since Pluk cannot look up the name before the answer, so a
 * number is labelled as one rather than shown as if it were a name. */
export function postingAsText(page: string): string {
  return /^\d+$/.test(page) ? `Posting as LinkedIn page ${page}` : `Posting as ${page}`;
}
