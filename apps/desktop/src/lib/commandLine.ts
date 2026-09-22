// A command as a person reads it — `module::command_line` of the backend, for the lines the
// UI draws from an argv: the Activity screen's record of what ran.

/**
 * The words joined, a word quoted unless it could not be misread, so that a path with a
 * space in it reads as one argument. For reading only: nothing parses it back, and the
 * process received the argv.
 */
export function commandLine(words: readonly string[]): string {
  return words
    .map((word) =>
      word !== '' && /^[A-Za-z0-9_./:=@%+,-]+$/.test(word)
        ? word
        : `'${word.split("'").join("'\\''")}'`,
    )
    .join(' ');
}
