import {
  configCandidates,
  parseTsconfigPaths,
  pickDefinition,
  resolveImport,
  tokenAt,
  type PathAlias,
} from './go-to-definition'

const files = new Set([
  'src/app.ts',
  'src/util.ts',
  'src/lib/index.ts',
  'src/components/Button.tsx',
  'src/data.json',
  'src/styles.css',
  'apps/desktop/src/App.tsx',
  'apps/desktop/src/stores/ui.ts',
])

describe('tokenAt', () => {
  it('finds the specifier of every import form and nothing around it', () => {
    const line = `import { x } from './util'`
    const hit = tokenAt(line, line.indexOf('util'))
    expect(hit).toEqual({ target: { kind: 'import', specifier: './util' }, from: line.indexOf('./util'), to: line.length - 1 })
    expect(tokenAt(`const m = require("../a/b")`, 20)?.target).toEqual({ kind: 'import', specifier: '../a/b' })
    expect(tokenAt(`const m = await import('./lazy')`, 28)?.target).toEqual({ kind: 'import', specifier: './lazy' })
    expect(tokenAt(`import './side-effect'`, 10)?.target).toEqual({ kind: 'import', specifier: './side-effect' })
    expect(tokenAt(`export * from "@/x"`, 17)?.target).toEqual({ kind: 'import', specifier: '@/x' })
  })

  it('finds the name under the cursor, including at the end of the word', () => {
    const line = '  const total = sumAll(items)'
    expect(tokenAt(line, line.indexOf('sumAll') + 2)?.target).toEqual({ kind: 'name', name: 'sumAll' })
    expect(tokenAt(line, line.indexOf('sumAll') + 6)?.target).toEqual({ kind: 'name', name: 'sumAll' })
    expect(tokenAt(line, line.indexOf('sumAll'))).toMatchObject({ from: line.indexOf('sumAll'), to: line.indexOf('sumAll') + 6 })
    expect(tokenAt('$store.get_value()', 3)?.target).toEqual({ kind: 'name', name: '$store' })
  })

  it('treats a name inside an import line as a name, outside the string', () => {
    const line = `import { Button } from './Button'`
    expect(tokenAt(line, 11)?.target).toEqual({ kind: 'name', name: 'Button' })
  })

  it('ignores one-character names, numbers, keywords and blank space', () => {
    expect(tokenAt('a + b', 0)).toBeNull()
    expect(tokenAt('x = 42', 5)).toBeNull()
    expect(tokenAt('const x = 1', 2)).toBeNull()
    expect(tokenAt('foo   bar', 5)).toBeNull()
    expect(tokenAt('', 0)).toBeNull()
  })

  it('keeps accented identifiers whole', () => {
    expect(tokenAt('const tiếng = 1', 8)?.target).toEqual({ kind: 'name', name: 'tiếng' })
  })
})

describe('resolveImport', () => {
  it('resolves relative paths with extensions, index files and parent segments', () => {
    expect(resolveImport(files, 'src/app.ts', './util')).toBe('src/util.ts')
    expect(resolveImport(files, 'src/app.ts', './lib')).toBe('src/lib/index.ts')
    expect(resolveImport(files, 'src/lib/index.ts', '../components/Button')).toBe('src/components/Button.tsx')
    expect(resolveImport(files, 'src/app.ts', './data.json')).toBe('src/data.json')
    expect(resolveImport(files, 'src/app.ts', './styles.css')).toBe('src/styles.css')
    expect(resolveImport(files, 'src/app.ts', '.')).toBeNull() // src/index.* does not exist
  })

  it('maps an ESM .js specifier onto the TypeScript source', () => {
    expect(resolveImport(files, 'src/app.ts', './util.js')).toBe('src/util.ts')
    expect(resolveImport(files, 'src/app.ts', './components/Button.jsx')).toBe('src/components/Button.tsx')
    expect(resolveImport(files, 'src/app.ts', './components/Card.js')).toBeNull()
    expect(resolveImport(files, 'src/lib/index.ts', '../components/Button.js')).toBe('src/components/Button.tsx')
  })

  it('returns null for packages, missing files and paths that leave the checkout', () => {
    expect(resolveImport(files, 'src/app.ts', 'react')).toBeNull()
    expect(resolveImport(files, 'src/app.ts', './missing')).toBeNull()
    expect(resolveImport(files, 'src/app.ts', '../../outside')).toBeNull()
  })

  it('resolves aliases, trying each target and requiring the whole pattern', () => {
    const aliases: PathAlias[] = [
      { pattern: '@/*', targets: ['apps/desktop/src/*'] },
      { pattern: '~util', targets: ['nowhere/x', 'src/util'] },
    ]
    expect(resolveImport(files, 'apps/desktop/src/App.tsx', '@/stores/ui', aliases)).toBe('apps/desktop/src/stores/ui.ts')
    expect(resolveImport(files, 'apps/desktop/src/App.tsx', '~util', aliases)).toBe('src/util.ts')
    expect(resolveImport(files, 'apps/desktop/src/App.tsx', '~util/extra', aliases)).toBeNull()
    expect(resolveImport(files, 'apps/desktop/src/App.tsx', '@/nope', aliases)).toBeNull()
    expect(resolveImport(files, 'apps/desktop/src/App.tsx', '@/stores/ui')).toBeNull() // no aliases given
  })
})

describe('tsconfig paths', () => {
  it('lists config files from the nearest directory to the root', () => {
    expect(configCandidates('apps/desktop/src/App.tsx')).toEqual([
      'apps/desktop/src/tsconfig.json',
      'apps/desktop/src/jsconfig.json',
      'apps/desktop/tsconfig.json',
      'apps/desktop/jsconfig.json',
      'apps/tsconfig.json',
      'apps/jsconfig.json',
      'tsconfig.json',
      'jsconfig.json',
    ])
    expect(configCandidates('index.ts')).toEqual(['tsconfig.json', 'jsconfig.json'])
  })

  it('reads paths from JSON with comments and trailing commas, relative to the config', () => {
    const text = `{
      // aliases
      "compilerOptions": {
        "strict": true, /* inline */
        "paths": { "@/*": ["./src/*"], "url": ["http://x//y"], },
      },
    }`
    expect(parseTsconfigPaths(text, 'apps/desktop/tsconfig.json')).toEqual([
      { pattern: '@/*', targets: ['apps/desktop/src/*'] },
      { pattern: 'url', targets: ['apps/desktop/http:/x/y'] },
    ])
  })

  it('applies baseUrl and skips malformed entries and configs', () => {
    expect(
      parseTsconfigPaths('{"compilerOptions":{"baseUrl":"src","paths":{"lib/*":["lib/*"],"bad":"x","empty":[]}}}', 'tsconfig.json'),
    ).toEqual([{ pattern: 'lib/*', targets: ['src/lib/*'] }])
    expect(parseTsconfigPaths('{"compilerOptions":{}}', 'tsconfig.json')).toEqual([])
    expect(parseTsconfigPaths('not json', 'tsconfig.json')).toEqual([])
    expect(parseTsconfigPaths('{"compilerOptions":{"paths":{"@/*":["../../x/*"]}}}', 'tsconfig.json')).toEqual([])
  })
})

describe('pickDefinition', () => {
  const hit = (path: string, line: number, text: string) => ({ path, line, text })

  it('recognises declarations across languages', () => {
    const declarations = [
      'export function sumAll(items: number[]) {',
      'export default async function sumAll() {',
      'function* sumAll() {',
      'const sumAll = (items) => items.length',
      'export const sumAll: Fn = async (a, b) => {',
      'sumAll: (a) => a,',
      'class sumAll extends Base {',
      'export interface sumAll {',
      'type sumAll = string',
      '  async sumAll(items: number[]): Promise<void> {',
      '  private static sumAll() {',
      'def sumAll(items):',
      'pub fn sumAll<T>(items: &[T]) -> usize {',
      'func sumAll(items []int) int {',
      'func (s *Store) sumAll(items []int) int {',
      'sumAll = function () {',
      'let sumAll',
    ]
    for (const text of declarations) {
      expect(pickDefinition([hit('a.ts', 3, text)], 'sumAll', 'other.ts'), text).toEqual({ path: 'a.ts', line: 3 })
    }
  })

  it('rejects uses, other identifiers and substrings', () => {
    const uses = [
      'const total = sumAll(items)',
      '  sumAll(items)',
      'return sumAll',
      'import { sumAll } from "./a"',
      'function sumAllItems() {',
      'function mysumAll() {',
      'const $sumAll = 1',
    ]
    for (const text of uses) expect(pickDefinition([hit('a.ts', 3, text)], 'sumAll', 'other.ts'), text).toBeNull()
  })

  it('escapes regex characters in the name', () => {
    expect(pickDefinition([hit('a.js', 1, 'const $store = 1')], '$store', 'a.js')).toEqual({ path: 'a.js', line: 1 })
    expect(pickDefinition([hit('a.js', 1, 'const xstore = 1')], '$store', 'a.js')).toBeNull()
  })

  it('prefers the open file, else a unique declaration, else asks for the search', () => {
    const elsewhere = hit('b.ts', 9, 'function run() {')
    const local = hit('a.ts', 4, 'function run() {')
    const localLater = hit('a.ts', 20, 'const run = () => 1')
    expect(pickDefinition([elsewhere, local, localLater], 'run', 'a.ts')).toEqual({ path: 'a.ts', line: 4 })
    expect(pickDefinition([hit('c.ts', 1, 'run()'), elsewhere], 'run', 'a.ts')).toEqual({ path: 'b.ts', line: 9 })
    expect(pickDefinition([elsewhere, hit('c.ts', 2, 'class run {')], 'run', 'a.ts')).toBeNull()
    expect(pickDefinition([hit('c.ts', 1, 'run()')], 'run', 'a.ts')).toBeNull()
    expect(pickDefinition([], 'run', 'a.ts')).toBeNull()
  })
})
