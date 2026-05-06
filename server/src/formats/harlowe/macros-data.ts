/**
 * Knot v2 — Harlowe 3 Data Macros
 *
 * Data structures, array operations, dataset operations, datamap operations,
 * string operations, and pattern matching.
 */

import type { MacroDef } from '../_types';
import { mc, sig, arg, MacroKind } from './macros-helpers';

export function getDataMacros(): MacroDef[] {
  return [

    // ─── DATA STRUCTURES ───────────────────────────────────────────
    mc('a:', 'dataStructure', MacroKind.Instant,
      'Creates an array from the given values.',
      [
        sig([arg('values', 'any', false, { variadic: true, description: 'Values to put in the array' })], 'Array'),
      ],
      { aliases: ['array:'] }),
    mc('dm:', 'dataStructure', MacroKind.Instant,
      'Creates a datamap from alternating key-value pairs.',
      [
        sig([arg('pairs', 'string|any', true, { variadic: true, description: 'Alternating key and value arguments' })], 'Datamap'),
      ],
      { aliases: ['datamap:'] }),
    mc('ds:', 'dataStructure', MacroKind.Instant,
      'Creates a dataset from the given values.',
      [
        sig([arg('values', 'any', false, { variadic: true, description: 'Values to put in the dataset' })], 'Dataset'),
      ],
      { aliases: ['dataset:'] }),

    // ─── ARRAY OPERATIONS ──────────────────────────────────────────
    mc('all-pass:', 'dataStructure', MacroKind.Instant,
      'Returns true if every element in the array passes the test lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Test lambda' }), arg('array', 'Array', true, { description: 'The array to test' })], 'boolean'),
      ],
      { aliases: ['pass:'] }),
    mc('altered:', 'dataStructure', MacroKind.Instant,
      'Returns a new array with each element transformed by the lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Transform lambda' }), arg('array', 'Array', true, { description: 'The array to transform' })], 'Array'),
      ]),
    mc('count:', 'dataStructure', MacroKind.Instant,
      'Returns the number of times a value appears in an array, or a pattern matches a string.',
      [
        sig([arg('haystack', 'Array|string', true, { description: 'The array or string to search' }), arg('needle', 'any|Datatype', true, { description: 'The value or pattern to count' })], 'number'),
      ]),
    mc('dm-altered:', 'dataStructure', MacroKind.Instant,
      'Returns a new datamap with values transformed by the lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Transform lambda (receives value)' }), arg('datamap', 'Datamap', true, { description: 'The datamap to transform' })], 'Datamap'),
      ],
      { aliases: ['datamap-altered:'] }),
    mc('dm-entries:', 'dataStructure', MacroKind.Instant,
      'Returns an array of the datamap\'s key-value pairs as 2-element arrays.',
      [
        sig([arg('datamap', 'Datamap', true, { description: 'The datamap' })], 'Array<Array>'),
      ],
      { aliases: ['data-entries:', 'datamap-entries:'] }),
    mc('dm-names:', 'dataStructure', MacroKind.Instant,
      'Returns an array of the datamap\'s key names.',
      [
        sig([arg('datamap', 'Datamap', true, { description: 'The datamap' })], 'Array<string>'),
      ],
      { aliases: ['data-names:', 'datamap-names:'] }),
    mc('dm-values:', 'dataStructure', MacroKind.Instant,
      'Returns an array of the datamap\'s values.',
      [
        sig([arg('datamap', 'Datamap', true, { description: 'The datamap' })], 'Array'),
      ],
      { aliases: ['data-values:', 'datamap-values:'] }),
    mc('find:', 'dataStructure', MacroKind.Instant,
      'Returns a new array of elements that pass the test lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Filter lambda' }), arg('array', 'Array', true, { description: 'The array to filter' })], 'Array'),
      ]),
    mc('folded:', 'dataStructure', MacroKind.Instant,
      'Reduces an array to a single value by repeatedly applying a lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Fold lambda (receives accumulator and element)' }), arg('array', 'Array', true, { description: 'The array to fold' })], 'any'),
        sig([arg('lambda', 'Lambda', true), arg('initial', 'any', true, { description: 'Initial accumulator value' }), arg('array', 'Array', true)], 'any'),
      ]),
    mc('interlaced:', 'dataStructure', MacroKind.Instant,
      'Returns a new array by interleaving elements from multiple arrays.',
      [
        sig([arg('arrays', 'Array', true, { variadic: true, description: 'Arrays to interleave' })], 'Array'),
      ]),
    mc('none-pass:', 'dataStructure', MacroKind.Instant,
      'Returns true if no elements in the array pass the test lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Test lambda' }), arg('array', 'Array', true, { description: 'The array to test' })], 'boolean'),
      ]),
    mc('permutations:', 'dataStructure', MacroKind.Instant,
      'Returns an array of all possible orderings (permutations) of the given array.',
      [
        sig([arg('array', 'Array', true, { description: 'The array to permute' })], 'Array<Array>'),
      ]),
    mc('range:', 'dataStructure', MacroKind.Instant,
      'Creates an array of integers from start to end inclusive.',
      [
        sig([arg('start', 'number', true, { description: 'Start value' }), arg('end', 'number', true, { description: 'End value' })], 'Array<number>'),
      ]),
    mc('repeated:', 'dataStructure', MacroKind.Instant,
      'Creates an array by repeating a value a given number of times.',
      [
        sig([arg('count', 'number', true, { description: 'Number of repetitions' }), arg('value', 'any', true, { description: 'Value to repeat' })], 'Array'),
      ]),
    mc('reversed:', 'dataStructure', MacroKind.Instant,
      'Returns a new array (or string) with elements in reverse order.',
      [
        sig([arg('collection', 'Array|string', true, { description: 'The array or string to reverse' })], 'Array|string'),
      ]),
    mc('rotated-to:', 'dataStructure', MacroKind.Instant,
      'Rotates the array so that the first element passing the test is at the front.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Test lambda' }), arg('array', 'Array', true, { description: 'The array to rotate' })], 'Array'),
      ]),
    mc('rotated:', 'dataStructure', MacroKind.Instant,
      'Rotates the array by the given number of positions.',
      [
        sig([arg('positions', 'number', true, { description: 'Number of positions to rotate (negative = left)' }), arg('array', 'Array', true, { description: 'The array to rotate' })], 'Array'),
      ]),
    mc('shuffled:', 'dataStructure', MacroKind.Instant,
      'Returns a new array with elements in random order.',
      [
        sig([arg('array', 'Array', true, { description: 'The array to shuffle' })], 'Array'),
      ]),
    mc('some-pass:', 'dataStructure', MacroKind.Instant,
      'Returns true if at least one element in the array passes the test lambda.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Test lambda' }), arg('array', 'Array', true, { description: 'The array to test' })], 'boolean'),
      ]),
    mc('sorted:', 'dataStructure', MacroKind.Instant,
      'Returns a new array with elements sorted in ascending order.',
      [
        sig([arg('array', 'Array', true, { description: 'The array to sort' })], 'Array'),
        sig([arg('lambda', 'Lambda', true, { description: 'Comparison lambda' }), arg('array', 'Array', true)], 'Array'),
      ]),
    mc('subarray:', 'dataStructure', MacroKind.Instant,
      'Returns a portion of the array from start index to end index.',
      [
        sig([arg('array', 'Array', true, { description: 'The source array' }), arg('start', 'number', true, { description: '1-based start index' }), arg('end', 'number', false, { description: '1-based end index (default: array length)' })], 'Array'),
      ]),
    mc('unique:', 'dataStructure', MacroKind.Instant,
      'Returns a new array with duplicate values removed.',
      [
        sig([arg('array', 'Array', true, { description: 'The array to deduplicate' })], 'Array'),
      ]),
    mc('unpack:', 'dataStructure', MacroKind.Instant,
      'Unpacks an array\'s values into separate variables, or a datamap\'s values into named variables.',
      [
        sig([arg('assignment', 'Lambda', true, { description: 'Unpacking expression' })], 'Instant'),
      ]),

    // ─── DATASET OPERATIONS ────────────────────────────────────────
    mc('ds-union:', 'dataStructure', MacroKind.Instant,
      'Returns a new dataset containing all values from both datasets, with duplicates removed.',
      [
        sig([arg('dataset1', 'Dataset', true, { description: 'First dataset' }), arg('dataset2', 'Dataset', true, { description: 'Second dataset' })], 'Dataset'),
      ],
      { aliases: ['dataset-union:'] }),
    mc('ds-intersect:', 'dataStructure', MacroKind.Instant,
      'Returns a new dataset containing only the values that exist in both datasets.',
      [
        sig([arg('dataset1', 'Dataset', true, { description: 'First dataset' }), arg('dataset2', 'Dataset', true, { description: 'Second dataset' })], 'Dataset'),
      ],
      { aliases: ['dataset-intersect:'] }),
    mc('ds-exclusion:', 'dataStructure', MacroKind.Instant,
      'Returns a new dataset containing values from the first dataset that are not in the second.',
      [
        sig([arg('dataset1', 'Dataset', true, { description: 'First dataset' }), arg('dataset2', 'Dataset', true, { description: 'Second dataset' })], 'Dataset'),
      ],
      { aliases: ['dataset-exclusion:'] }),

    // ─── STRING OPERATIONS ─────────────────────────────────────────
    mc('str:', 'string', MacroKind.Instant,
      'Converts values to strings, or concatenates multiple values into a single string.',
      [
        sig([arg('values', 'any', true, { variadic: true, description: 'Values to convert/concatenate' })], 'string'),
      ],
      { aliases: ['string:', 'text:'] }),
    mc('digit-format:', 'string', MacroKind.Instant,
      'Formats a number as a string with the given number of decimal places.',
      [
        sig([arg('places', 'number', true, { description: 'Number of decimal places' }), arg('number', 'number', true, { description: 'The number to format' })], 'string'),
      ]),
    mc('joined:', 'string', MacroKind.Instant,
      'Joins an array of strings into a single string with a separator.',
      [
        sig([arg('separator', 'string', true, { description: 'The separator string' }), arg('array', 'Array', true, { description: 'The array to join' })], 'string'),
      ]),
    mc('lowercase:', 'string', MacroKind.Instant,
      'Converts a string to lowercase.',
      [
        sig([arg('string', 'string', true, { description: 'The string' })], 'string'),
      ]),
    mc('lowerfirst:', 'string', MacroKind.Instant,
      'Converts the first character of a string to lowercase.',
      [
        sig([arg('string', 'string', true, { description: 'The string' })], 'string'),
      ]),
    mc('plural:', 'string', MacroKind.Instant,
      'Returns the plural form of a string, or appends "s" if no special form is known.',
      [
        sig([arg('string', 'string', true, { description: 'The string' })], 'string'),
      ]),
    mc('source:', 'string', MacroKind.Instant,
      'Returns the source code representation of a changer, colour, or other value.',
      [
        sig([arg('value', 'any', true, { description: 'The value' })], 'string'),
      ]),
    mc('split:', 'string', MacroKind.Instant,
      'Splits a string by a separator into an array of substrings.',
      [
        sig([arg('separator', 'string', true, { description: 'The separator' }), arg('string', 'string', true, { description: 'The string to split' })], 'Array<string>'),
      ],
      { aliases: ['splitted:'] }),
    mc('str-find:', 'string', MacroKind.Instant,
      'Finds all matches of a pattern in a string, returning an array of matching strings.',
      [
        sig([arg('pattern', 'string|Datatype', true, { description: 'Pattern to find' }), arg('string', 'string', true, { description: 'The string to search' })], 'Array<string>'),
      ],
      { aliases: ['string-find:'] }),
    mc('str-nth:', 'string', MacroKind.Instant,
      'Returns the Nth match of a pattern in a string.',
      [
        sig([arg('pattern', 'string|Datatype', true, { description: 'Pattern to find' }), arg('index', 'number', true, { description: '1-based match index' }), arg('string', 'string', true, { description: 'The string to search' })], 'string'),
      ],
      { aliases: ['string-nth:'] }),
    mc('str-repeated:', 'string', MacroKind.Instant,
      'Returns a string repeated the given number of times.',
      [
        sig([arg('count', 'number', true, { description: 'Number of repetitions' }), arg('string', 'string', true, { description: 'The string to repeat' })], 'string'),
      ],
      { aliases: ['string-repeated:'] }),
    mc('str-replaced:', 'string', MacroKind.Instant,
      'Returns a string with all matches of a pattern replaced by another string.',
      [
        sig([arg('pattern', 'string|Datatype', true, { description: 'Pattern to find' }), arg('replacement', 'string', true, { description: 'Replacement string' }), arg('string', 'string', true, { description: 'The string to search' })], 'string'),
      ],
      { aliases: ['string-replaced:', 'replaced:'] }),
    mc('str-reversed:', 'string', MacroKind.Instant,
      'Returns a string with characters in reverse order.',
      [
        sig([arg('string', 'string', true, { description: 'The string to reverse' })], 'string'),
      ],
      { aliases: ['string-reversed:'] }),
    mc('substring:', 'string', MacroKind.Instant,
      'Returns a portion of a string from start to end index.',
      [
        sig([arg('string', 'string', true, { description: 'The source string' }), arg('start', 'number', true, { description: '1-based start index' }), arg('end', 'number', false, { description: '1-based end index' })], 'string'),
      ]),
    mc('trimmed:', 'string', MacroKind.Instant,
      'Returns a string with leading and trailing whitespace removed.',
      [
        sig([arg('string', 'string', true, { description: 'The string to trim' })], 'string'),
      ]),
    mc('uppercase:', 'string', MacroKind.Instant,
      'Converts a string to uppercase.',
      [
        sig([arg('string', 'string', true, { description: 'The string' })], 'string'),
      ]),
    mc('upperfirst:', 'string', MacroKind.Instant,
      'Converts the first character of a string to uppercase.',
      [
        sig([arg('string', 'string', true, { description: 'The string' })], 'string'),
      ]),
    mc('words:', 'string', MacroKind.Instant,
      'Splits a string into an array of words (by whitespace).',
      [
        sig([arg('string', 'string', true, { description: 'The string to split' })], 'Array<string>'),
      ]),
    mc('char:', 'string', MacroKind.Instant,
      'Returns the character at the given 1-based index in a string.',
      [
        sig([arg('index', 'number', true, { description: '1-based character index' }), arg('string', 'string', true, { description: 'The source string' })], 'string'),
      ]),
    mc('startcase:', 'string', MacroKind.Instant,
      'Converts a string to start case (also known as title case), capitalizing the first letter of each word.',
      [
        sig([arg('string', 'string', true, { description: 'The string to convert' })], 'string'),
      ]),

    // ─── PATTERN MATCHING ──────────────────────────────────────────
    mc('p:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches a specific value or datatype.',
      [
        sig([arg('value', 'any|Datatype', true, { description: 'The value or datatype to match' })], 'Pattern'),
      ],
      { aliases: ['pattern:'] }),
    mc('p-either:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches any of the given sub-patterns.',
      [
        sig([arg('patterns', 'Pattern', true, { variadic: true, description: 'Sub-patterns to match against' })], 'Pattern'),
      ],
      { aliases: ['pattern-either:'] }),
    mc('p-opt:', 'pattern', MacroKind.Instant,
      'Creates a pattern that optionally matches the given sub-pattern.',
      [
        sig([arg('pattern', 'Pattern', true, { description: 'The optional sub-pattern' })], 'Pattern'),
      ],
      { aliases: ['p-optional:', 'pattern-opt:', 'pattern-optional:'] }),
    mc('p-many:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches the given sub-pattern repeated one or more times.',
      [
        sig([arg('pattern', 'Pattern', true, { description: 'The sub-pattern to repeat' })], 'Pattern'),
        sig([arg('min', 'number', true, { description: 'Minimum repetitions' }), arg('max', 'number', true, { description: 'Maximum repetitions' }), arg('pattern', 'Pattern', true)], 'Pattern'),
      ],
      { aliases: ['pattern-many:'] }),
    mc('p-not:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches anything NOT matching the given sub-pattern.',
      [
        sig([arg('pattern', 'Pattern', true, { description: 'The sub-pattern to negate' })], 'Pattern'),
      ],
      { aliases: ['pattern-not:'] }),
    mc('p-before:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches content before the given sub-pattern.',
      [
        sig([arg('pattern', 'Pattern', true, { description: 'The boundary sub-pattern' })], 'Pattern'),
      ],
      { aliases: ['pattern-before:'] }),
    mc('p-not-before:', 'pattern', MacroKind.Instant,
      'Creates a pattern that matches content not before the given sub-pattern.',
      [
        sig([arg('pattern', 'Pattern', true, { description: 'The boundary sub-pattern' })], 'Pattern'),
      ],
      { aliases: ['pattern-not-before:'] }),
    mc('p-start:', 'pattern', MacroKind.Instant,
      'Anchors a pattern to match only at the start of the string.',
      [
        sig([], 'Pattern', 'Use as a modifier within a p: pattern.'),
      ],
      { aliases: ['pattern-start:'] }),
    mc('p-end:', 'pattern', MacroKind.Instant,
      'Anchors a pattern to match only at the end of the string.',
      [
        sig([], 'Pattern', 'Use as a modifier within a p: pattern.'),
      ],
      { aliases: ['pattern-end:'] }),
    mc('p-ins:', 'pattern', MacroKind.Instant,
      'Makes the preceding pattern match case-insensitively.',
      [
        sig([], 'Pattern', 'Use as a modifier within a p: pattern.'),
      ],
      { aliases: ['p-insensitive:', 'pattern-ins:', 'pattern-insensitive:'] }),
  ];
}
