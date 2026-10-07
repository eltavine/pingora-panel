import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { i18n, setLocale } from '@/i18n'
import ErrorPagesEditor from '../ErrorPagesEditor.vue'
import { newPage, type PagesForm } from '../pages'

function host(form: PagesForm) {
  return defineComponent({
    setup() {
      const model = ref(form)
      return () =>
        h(ErrorPagesEditor, {
          modelValue: model.value,
          'onUpdate:modelValue': (value: PagesForm) => {
            model.value = value
          },
          idPrefix: 'page',
        })
    },
  })
}

afterEach(() => {
  document.body.innerHTML = ''
})

describe('ErrorPagesEditor', () => {
  beforeAll(() => setLocale('en'))

  it("shows each page with its kind's fields and what keeps it from being saved", async () => {
    const wrapper = mount(
      host({
        pages: [
          { ...newPage(404), body: '<h1>Gone</h1>' },
          { ...newPage(302), kind: 'file', path: 'errors/302.html' },
          { ...newPage(503), kind: 'redirect', location: 'https://status.example/' },
        ],
        intercept: true,
      }),
      { attachTo: document.body, global: { plugins: [i18n] } },
    )
    await flushPromises()

    expect(wrapper.find('[data-page="body"] textarea').element).toHaveProperty(
      'value',
      '<h1>Gone</h1>',
    )
    expect(wrapper.find('#page-1-path').exists()).toBe(true)
    expect(wrapper.find('#page-2-location').exists()).toBe(true)
    expect(wrapper.find('#page-2-status').exists()).toBe(false)
    expect(wrapper.findAll('[role="alert"]').map((alert) => alert.text())).toEqual([
      '302 is not an error status (400 to 599).',
    ])
    expect(wrapper.text()).toContain("Replace upstreams' error responses too")

    await wrapper.find('[aria-label="Remove page 2"]').trigger('click')
    expect(wrapper.findAll('[data-page]')).toHaveLength(2)
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
    wrapper.unmount()
  })
})
