/**
 * Source-text helpers shared by the lint scripts.
 *
 * `stripComments` exists because the checks below are textual, and a comment is
 * not code. Documenting an example such as `t('rules.add')`, or quoting a
 * Chinese string inside an explanatory comment, would otherwise be reported as a
 * violation — a failure nobody can fix, which is how a linter gets switched off
 * instead of obeyed.
 *
 * It is deliberately not a parser. It understands just enough to avoid the
 * mistakes that matter: `//` inside a URL is not a comment, an escape sequence
 * does not end a string, and a block comment does not end at the first line that
 * happens to start with `*`.
 *
 * Blanking rather than deleting is intentional: the result has the same length
 * and the same number of newlines as the input, so reported line numbers stay
 * correct.
 */
export function stripComments(source) {
  let out = '';
  let quote = null;
  let i = 0;
  const n = source.length;

  while (i < n) {
    const char = source[i];
    const next = source[i + 1];

    if (quote) {
      out += char;
      if (char === '\\') {
        out += next ?? '';
        i += 2;
        continue;
      }
      if (char === quote) quote = null;
      i += 1;
      continue;
    }

    if (char === '"' || char === "'" || char === '`') {
      quote = char;
      out += char;
      i += 1;
      continue;
    }

    if (char === '/' && next === '*') {
      const end = source.indexOf('*/', i + 2);
      const stop = end === -1 ? n : end + 2;
      out += source.slice(i, stop).replace(/[^\n]/g, ' ');
      i = stop;
      continue;
    }

    if (char === '/' && next === '/') {
      // `//` only starts a comment when it is not part of a scheme such as
      // `https://`, which in practice means "not preceded by a colon".
      if (source[i - 1] !== ':') {
        const end = source.indexOf('\n', i);
        const stop = end === -1 ? n : end;
        out += ' '.repeat(stop - i);
        i = stop;
        continue;
      }
    }

    out += char;
    i += 1;
  }

  return out;
}

/**
 * Tokenises TypeScript / Vue script source into strings, identifiers and
 * punctuation. Enough to walk an object literal without writing a parser.
 */
export function tokenize(source) {
  const tokens = [];
  let i = 0;
  const n = source.length;

  while (i < n) {
    const char = source[i];

    if (/\s/.test(char)) {
      i += 1;
      continue;
    }

    if (char === '"' || char === "'" || char === '`') {
      let value = '';
      let j = i + 1;
      while (j < n) {
        if (source[j] === '\\') {
          value += source[j] + (source[j + 1] ?? '');
          j += 2;
          continue;
        }
        if (source[j] === char) break;
        value += source[j];
        j += 1;
      }
      tokens.push({ type: 'string', value, index: i });
      i = j + 1;
      continue;
    }

    if (/[A-Za-z_$]/.test(char)) {
      let j = i;
      while (j < n && /[\w$]/.test(source[j])) j += 1;
      tokens.push({ type: 'ident', value: source.slice(i, j), index: i });
      i = j;
      continue;
    }

    tokens.push({ type: 'punct', value: char, index: i });
    i += 1;
  }

  return tokens;
}
