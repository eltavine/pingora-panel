import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { features, navigationGroups } from '@/features'
import type { NavigationItem } from '@/features/types'
import { useSession } from '@/lib/session'

/** Destinations of the navigation bar besides *More* (ADR 0023). */
const NAVIGATION_BAR_SIZE = 4

/** The destinations the account may open, grouped and for the navigation bar. */
export function useNavigation() {
  const route = useRoute()
  const { can } = useSession()

  const permitted = computed(() =>
    features.map((feature) => ({
      group: feature.group,
      items: feature.navigation.filter((item) => {
        const permission = item.permission ?? feature.permission
        return !permission || can(permission)
      }),
    })),
  )

  const groups = computed(() =>
    navigationGroups
      .map((group) => ({
        ...group,
        items: permitted.value
          .filter((feature) => feature.group === group.id)
          .flatMap((feature) => feature.items),
      }))
      .filter((group) => group.items.length > 0),
  )

  const bar = computed(() =>
    permitted.value
      .flatMap((feature) => feature.items)
      .filter((item): item is NavigationItem & { navigationBar: number } =>
        Number.isFinite(item.navigationBar),
      )
      .sort((a, b) => a.navigationBar - b.navigationBar)
      .slice(0, NAVIGATION_BAR_SIZE),
  )

  function isActive(to: string) {
    return to === '/' ? route.path === '/' : route.path === to || route.path.startsWith(`${to}/`)
  }

  return { groups, bar, isActive }
}
