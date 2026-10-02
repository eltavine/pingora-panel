import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import StatusIndicator, { type StatusTone } from '../StatusIndicator.vue'

const tones: StatusTone[] = ['positive', 'negative', 'warning', 'pending', 'neutral']

describe('StatusIndicator', () => {
  it('pairs every tone with a distinct glyph and visible text', () => {
    const glyphs = new Set<string>()
    for (const tone of tones) {
      const wrapper = mount(StatusIndicator, { props: { tone, label: `state ${tone}` } })
      const icon = wrapper.find('svg')

      expect(wrapper.attributes('role')).toBe('status')
      expect(wrapper.text()).toBe(`state ${tone}`)
      expect(icon.exists()).toBe(true)
      expect(icon.attributes('aria-hidden')).toBe('true')
      glyphs.add(icon.html())
    }
    expect(glyphs.size).toBe(tones.length)
  })
})
