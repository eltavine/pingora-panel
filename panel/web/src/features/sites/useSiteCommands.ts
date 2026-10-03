import { computed } from 'vue'
import { useMutation } from '@tanstack/vue-query'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import {
  exportSites,
  validation,
  type BatchAction,
  type SiteBundle,
  type SiteView,
} from '@/api/generated'
import {
  batchSitesMutation,
  cloneSiteMutation,
  deleteSiteMutation,
  disableSiteMutation,
  enableSiteMutation,
  favoriteSiteMutation,
  importSitesMutation,
  restoreSiteMutation,
  unfavoriteSiteMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import {
  changeHeaders,
  notifyFailure,
  plainHeaders,
  useRefreshConfiguration,
} from '@/lib/configuration'
import { downloadJson } from '@/lib/download'

/** Site commands shared by the list and the detail page. */
export function useSiteCommands() {
  const { t } = useI18n()
  const refresh = useRefreshConfiguration()
  const enable = useMutation(enableSiteMutation())
  const disable = useMutation(disableSiteMutation())
  const favorite = useMutation(favoriteSiteMutation())
  const unfavorite = useMutation(unfavoriteSiteMutation())
  const clone = useMutation(cloneSiteMutation())
  const remove = useMutation(deleteSiteMutation())
  const restore = useMutation(restoreSiteMutation())
  const batch = useMutation(batchSitesMutation())
  const importer = useMutation(importSitesMutation())
  const mutations = [enable, disable, favorite, unfavorite, clone, remove, restore, batch, importer]
  const busy = computed(() => mutations.some((mutation) => mutation.isPending.value))

  const onError = (error: unknown) => notifyFailure(error, t('common.changeFailed'))
  const done = (message?: string) => () => {
    if (message) {
      toast.success(message)
    }
    void refresh()
  }

  return {
    busy,
    setEnabled(site: SiteView, enabled: boolean) {
      const mutation = enabled ? enable : disable
      mutation.mutate(
        { path: { id: site.id }, headers: plainHeaders() },
        { onSuccess: done(), onError },
      )
    },
    setFavorite(site: SiteView, value: boolean) {
      const mutation = value ? favorite : unfavorite
      mutation.mutate(
        { path: { id: site.id }, headers: plainHeaders() },
        { onSuccess: done(), onError },
      )
    },
    clone(site: SiteView, onCloned?: (clone: SiteView) => void) {
      const name = t('sites.cloneSuffix', { name: site.name })
      clone.mutate(
        { path: { id: site.id }, body: { name }, headers: plainHeaders() },
        {
          onSuccess: (created) => {
            done(t('sites.cloned', { name: created.name }))()
            onCloned?.(created)
          },
          onError,
        },
      )
    },
    remove(site: SiteView, permanent: boolean, onRemoved?: () => void) {
      remove.mutate(
        {
          path: { id: site.id },
          query: { permanent },
          headers: changeHeaders(site.etag),
        },
        {
          onSuccess: () => {
            done(permanent ? t('sites.purged') : t('sites.deleted'))()
            onRemoved?.()
          },
          onError,
        },
      )
    },
    restore(site: SiteView) {
      restore.mutate(
        { path: { id: site.id }, headers: plainHeaders() },
        { onSuccess: done(t('sites.restored')), onError },
      )
    },
    batch(action: BatchAction, ids: string[], onDone?: () => void) {
      batch.mutate(
        { body: { action, ids }, headers: plainHeaders() },
        {
          onSuccess: (sites) => {
            done(t('sites.batch.done', { count: action === 'purge' ? ids.length : sites.length }))()
            onDone?.()
          },
          onError,
        },
      )
    },
    async importFile(file: File) {
      let bundle: SiteBundle
      try {
        bundle = JSON.parse(await file.text()) as SiteBundle
      } catch (error) {
        notifyFailure(error, t('common.changeFailed'))
        return
      }
      importer.mutate(
        { body: bundle, headers: plainHeaders() },
        {
          onSuccess: (result) => done(t('sites.imported', { count: result.created.length }))(),
          onError,
        },
      )
    },
    async exportSites(ids: string[]) {
      try {
        const { data } = await exportSites({
          query: ids.length > 0 ? { ids: ids.join(',') } : {},
          throwOnError: true,
        })
        const stamp = new Date().toISOString().slice(0, 10)
        downloadJson(`sites-${stamp}.json`, data)
        toast.success(t('sites.exported', { count: data.sites.length }))
      } catch (error) {
        notifyFailure(error, t('common.changeFailed'))
      }
    },
    async validate(ids: string[]) {
      try {
        const { data } = await validation({
          query: { site_ids: ids.join(',') },
          throwOnError: true,
        })
        if (data.valid) {
          toast.success(t('sites.validated'))
        } else {
          const messages = data.diagnostics.map((item) => String(item.message ?? ''))
          toast.warning(t('sites.validateFailed', { count: messages.length }), {
            description: messages.slice(0, 5).join('\n'),
          })
        }
      } catch (error) {
        notifyFailure(error, t('common.changeFailed'))
      }
    },
  }
}
