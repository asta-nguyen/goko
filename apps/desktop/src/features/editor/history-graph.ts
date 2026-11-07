import type { GitViewCommit } from '../../services/types'

/** Branch-line colours at mid luminance so they read on light and dark backgrounds. */
export const GRAPH_PALETTE = [
  'hsl(0, 75%, 55%)',
  'hsl(30, 90%, 55%)',
  'hsl(50, 95%, 48%)',
  'hsl(145, 55%, 40%)',
  'hsl(190, 80%, 42%)',
  'hsl(220, 85%, 58%)',
  'hsl(275, 65%, 58%)',
  'hsl(335, 75%, 58%)',
] as const

/** A curved edge from a commit node down into a parent's lane. */
export interface GraphEdge {
  /** Lane the edge leaves from — the commit's own lane. */
  fromLane: number
  /** Lane the parent's line occupies below this row. */
  toLane: number
  /** Palette index colouring the edge and the line it joins. */
  color: number
}

/** One commit's graph geometry; pure data for the SVG renderer, no JSX. */
export interface GraphRow {
  /** Lane index of the commit's node. */
  lane: number
  /** Palette index of the commit's branch line. */
  color: number
  /** Whether the commit is the checkout's HEAD. */
  head: boolean
  /** Whether the commit has two or more parents. */
  merge: boolean
  /** Whether a line already waits for this commit, so one enters its node from above. */
  incoming: boolean
  /** Other lanes whose lines pass vertically through this row. */
  through: { lane: number; color: number }[]
  /** One edge per parent, in parent order. */
  edges: GraphEdge[]
}

// A lane holds the commit id expected next plus the colour of its line;
// parents fan out into free lanes, each taking the next colour in rotation.
export function layoutGraph(commits: GitViewCommit[]): { rows: GraphRow[]; lanes: number } {
  const expected: (string | null)[] = []
  const colors: number[] = []
  let nextColor = 0
  let laneCount = 0

  const rows = commits.map((commit) => {
    const before = [...expected]
    let lane = expected.indexOf(commit.id)
    const incoming = lane >= 0
    let color: number
    if (lane < 0) {
      color = nextColor
      nextColor = (nextColor + 1) % GRAPH_PALETTE.length
      lane = expected.indexOf(null)
      if (lane < 0) {
        expected.push(commit.id)
        colors.push(color)
        lane = expected.length - 1
      } else {
        expected[lane] = commit.id
        colors[lane] = color
      }
    } else {
      color = colors[lane]
    }
    // A first parent that another lane already waits for is joined by an edge,
    // not tracked twice; a duplicate lane would never be freed.
    const [first, ...rest] = commit.parents
    expected[lane] = first !== undefined && !expected.includes(first) ? first : null
    for (const parent of rest) {
      if (expected.includes(parent)) continue
      const parentColor = nextColor
      nextColor = (nextColor + 1) % GRAPH_PALETTE.length
      const free = expected.indexOf(null)
      if (free < 0) {
        expected.push(parent)
        colors.push(parentColor)
      } else {
        expected[free] = parent
        colors[free] = parentColor
      }
    }
    while (expected.length > 0 && expected.at(-1) === null) {
      expected.pop()
      colors.pop()
    }
    const after = [...expected]
    laneCount = Math.max(laneCount, before.length, after.length, lane + 1)
    const through: GraphRow['through'] = []
    for (let index = 0; index < after.length; index++) {
      if (index === lane || before[index] == null || after[index] == null) continue
      through.push({ lane: index, color: colors[index] })
    }
    const edges: GraphEdge[] = []
    for (const parent of commit.parents) {
      const toLane = after.indexOf(parent)
      if (toLane < 0) continue
      edges.push({ fromLane: lane, toLane, color: colors[toLane] })
    }
    return { lane, color, head: commit.head, merge: commit.parents.length > 1, incoming, through, edges }
  })

  return { rows, lanes: laneCount }
}
