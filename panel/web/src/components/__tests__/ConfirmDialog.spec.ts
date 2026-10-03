import { flushPromises, mount } from '@vue/test-utils'
import { Trash2 } from '@lucide/vue'
import { afterEach, describe, expect, it } from 'vitest'
import { computed, defineComponent, h, ref } from 'vue'
import { i18n } from '@/i18n'
import ConfirmDialog from '../ConfirmDialog.vue'

function host(confirmed: (string | null)[]) {
  return defineComponent({
    setup() {
      const target = ref<string | null>('alpha')
      const open = computed({
        get: () => target.value !== null,
        set: (value: boolean) => {
          if (!value) {
            target.value = null
          }
        },
      })
      return () =>
        h(ConfirmDialog, {
          open: open.value,
          'onUpdate:open': (value: boolean) => {
            open.value = value
          },
          icon: Trash2,
          title: 'Delete alpha',
          description: 'This cannot be undone.',
          confirmLabel: 'Delete',
          destructive: true,
          onConfirm: () => confirmed.push(target.value),
        })
    },
  })
}

function button(name: string) {
  const found = Array.from(document.body.querySelectorAll('button')).find(
    (candidate) => candidate.textContent?.trim() === name,
  )
  if (!found) {
    throw new Error(`no button named ${name}`)
  }
  return found
}

afterEach(() => {
  document.body.innerHTML = ''
})

describe('ConfirmDialog', () => {
  it('confirms what it shows before it closes', async () => {
    const confirmed: (string | null)[] = []
    const wrapper = mount(host(confirmed), { attachTo: document.body, global: { plugins: [i18n] } })
    await flushPromises()

    button('Delete').click()
    await flushPromises()

    expect(confirmed).toEqual(['alpha'])
    expect(document.body.querySelector('[role="alertdialog"]')).toBeNull()
    wrapper.unmount()
  })

  it('cancels without confirming', async () => {
    const confirmed: (string | null)[] = []
    const wrapper = mount(host(confirmed), { attachTo: document.body, global: { plugins: [i18n] } })
    await flushPromises()

    button(i18n.global.t('common.cancel')).click()
    await flushPromises()

    expect(confirmed).toEqual([])
    expect(document.body.querySelector('[role="alertdialog"]')).toBeNull()
    wrapper.unmount()
  })
})
