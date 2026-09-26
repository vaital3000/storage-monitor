// A fact of the detail panel in words, by its type: the one place a module's typed value
// becomes text, so every module's facts read alike.

import { formatBytes, formatStampDate } from './format';
import type { FactValue } from './ipc';

export function describeFact(value: FactValue): string {
  switch (value.type) {
    case 'text':
    case 'path':
      return value.value;
    case 'bytes':
      return formatBytes(value.value);
    case 'count':
      return value.value.toLocaleString('en-US');
    case 'date':
      return formatStampDate(value.value);
    case 'flag':
      return value.value ? 'Yes' : 'No';
  }
}
