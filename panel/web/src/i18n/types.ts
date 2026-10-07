import type { Locale } from './locales'

/** A tree of messages whose every leaf is a string, so each language defines the same keys. */
export type DeepString<T> = { [K in keyof T]: T[K] extends string ? string : DeepString<T[K]> }

/**
 * The messages a feature brings for its own pages, by language. They are
 * fetched when one of its pages opens, and merged into the shared ones.
 */
export type FeatureMessages = Record<Locale, () => Promise<{ default: object }>>
