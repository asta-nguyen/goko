import type { GitViewCommit } from '../../services/types'
import { GRAPH_PALETTE, layoutGraph } from './history-graph'

const commit = (id: string, parents: string[] = [], head = false): GitViewCommit => ({
  id,
  parents,
  refs: [],
  head,
  author: 'A U Thor',
  time: 0,
  subject: `commit ${id}`,
})

describe('layoutGraph', () => {
  it('keeps one colour along a first-parent line', () => {
    const { rows, lanes } = layoutGraph([
      commit('a', ['b']),
      commit('b', ['c']),
      commit('c', ['d']),
      commit('d'),
    ])
    expect(GRAPH_PALETTE).toHaveLength(8)
    expect(lanes).toBe(1)
    expect(rows.map((row) => row.lane)).toEqual([0, 0, 0, 0])
    expect(rows.map((row) => row.color)).toEqual([0, 0, 0, 0])
    expect(rows[0].edges).toEqual([{ fromLane: 0, toLane: 0, color: 0 }])
    expect(rows[0].through).toEqual([])
    expect(rows[3].edges).toEqual([])
  })

  it('gives a merge\'s second parent a new colour', () => {
    const { rows, lanes } = layoutGraph([
      commit('m', ['p1', 'p2']),
      commit('p1', ['x']),
      commit('p2', ['y']),
      commit('x'),
      commit('y'),
    ])
    expect(lanes).toBe(2)
    expect(rows[0].merge).toBe(true)
    expect(rows[0].edges).toEqual([
      { fromLane: 0, toLane: 0, color: 0 },
      { fromLane: 0, toLane: 1, color: 1 },
    ])
    expect(rows[1].lane).toBe(0)
    expect(rows[1].color).toBe(0)
    expect(rows[2].lane).toBe(1)
    expect(rows[2].color).toBe(1)
    expect(rows[4].lane).toBe(1)
    expect(rows[4].color).toBe(1)
  })

  it('reuses a freed lane but takes the next colour', () => {
    const { rows } = layoutGraph([
      commit('m', ['p1', 'p2']),
      commit('p1', ['x']),
      commit('p2'),
      commit('u'),
      commit('x'),
    ])
    expect(rows[2].lane).toBe(1)
    expect(rows[2].color).toBe(1)
    expect(rows[3].lane).toBe(1)
    expect(rows[3].color).toBe(2)
  })

  it('root commit ends its line', () => {
    const { rows, lanes } = layoutGraph([commit('a', ['b']), commit('b')])
    expect(rows[1].lane).toBe(0)
    expect(rows[1].color).toBe(0)
    expect(rows[1].edges).toEqual([])
    expect(rows[1].through).toEqual([])
    expect(lanes).toBe(1)
  })

  it('flags head and merge', () => {
    const { rows } = layoutGraph([
      commit('h', ['m'], true),
      commit('m', ['r', 's']),
      commit('r'),
      commit('s'),
    ])
    expect(rows.map((row) => [row.head, row.merge])).toEqual([
      [true, false],
      [false, true],
      [false, false],
      [false, false],
    ])
  })

  it('parent outside the loaded page leaves an open edge to the last row', () => {
    const { rows, lanes } = layoutGraph([commit('a', ['b', 'x']), commit('b')])
    expect(lanes).toBe(2)
    expect(rows[0].edges).toEqual([
      { fromLane: 0, toLane: 0, color: 0 },
      { fromLane: 0, toLane: 1, color: 1 },
    ])
    expect(rows[1].through).toEqual([{ lane: 1, color: 1 }])
  })
  it('curves a tip into the lane that already waits for its parent', () => {
    // a and b are separate tips that both descend from p.
    const { rows, lanes } = layoutGraph([commit('a', ['p']), commit('b', ['p']), commit('p', ['q']), commit('q')])
    expect(lanes).toBe(2)
    expect(rows[1].lane).toBe(1)
    // b's first parent is already tracked in lane 0: its edge curves there and lane 1 is not kept open.
    expect(rows[1].edges).toEqual([{ fromLane: 1, toLane: 0, color: rows[0].color }])
    expect(rows[2].lane).toBe(0)
    expect(rows[2].through).toEqual([])
    expect(rows[3].through).toEqual([])
  })

  it('does not keep a second lane open when two merge parents share a base', () => {
    // a's merge brings p1 and p2; both lines continue to the shared base.
    const { rows } = layoutGraph([commit('a', ['p1', 'p2']), commit('p1', ['base']), commit('p2', ['base']), commit('base')])
    expect(rows[2].edges).toEqual([{ fromLane: 1, toLane: 0, color: rows[0].edges[0].color }])
    expect(rows[3].lane).toBe(0)
    expect(rows[3].through).toEqual([])
  })

  it('flags whether a line enters the commit from above', () => {
    const { rows } = layoutGraph([commit('a', ['b']), commit('b')])
    expect(rows.map((row) => row.incoming)).toEqual([false, true])
  })

})
