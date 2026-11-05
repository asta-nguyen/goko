import { render } from '@testing-library/react'
import type { Activity } from '../../services/activity'
import { StateDot } from './StateBadge'

const dot = (container: HTMLElement) => {
  const el = container.firstElementChild
  if (!(el instanceof HTMLElement)) throw new Error('StateDot did not render an element')
  return el
}

describe('StateDot', () => {
  it.each<[Activity, string, boolean]>([
    ['working', 'bg-warning', true],
    ['waiting', 'bg-danger', false],
    ['done', 'bg-success', false],
  ])('running with %s activity recolours the dot and names it in the title', (activity, tone, pulse) => {
    const { container } = render(<StateDot state="running" activity={activity} />)

    const el = dot(container)
    expect(el.classList.contains(tone)).toBe(true)
    expect(el.classList.contains('animate-pulse')).toBe(pulse)
    expect(el.getAttribute('title')).toBe(`running · ${activity}`)
    expect(el.textContent).toBe(`running · ${activity}`)
  })

  it('renders a running session without activity exactly as before', () => {
    const { container } = render(<StateDot state="running" />)

    const el = dot(container)
    expect(el.classList.contains('bg-success')).toBe(true)
    expect(el.classList.contains('animate-pulse')).toBe(false)
    expect(el.getAttribute('title')).toBe('running')
    expect(el.textContent).toBe('running')
  })

  it.each([
    ['exited', 'bg-muted'],
    ['failed', 'bg-danger'],
    ['starting', 'bg-warning'],
  ] as const)('ignores activity for a %s session', (state, tone) => {
    const { container } = render(<StateDot state={state} activity="working" />)

    const el = dot(container)
    expect(el.classList.contains(tone)).toBe(true)
    expect(el.classList.contains('animate-pulse')).toBe(false)
    expect(el.getAttribute('title')).toBe(state)
    expect(el.textContent).toBe(state)
  })
})
