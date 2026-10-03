<script setup lang="ts">
import { computed } from 'vue'
import { areaPath, linePath, scaleOf } from './presentation'

/** One line of the chart; the first is filled. */
export interface ChartSeries {
  name: string
  values: (number | null)[]
  tone: 'foreground' | 'destructive'
}

const WIDTH = 100
const HEIGHT = 40

const props = defineProps<{
  /** What the chart shows, for assistive technology. */
  label: string
  /** The times of the points, already formatted. */
  times: string[]
  series: ChartSeries[]
  format: (value: number) => string
}>()

const scale = computed(() => scaleOf(props.series.map((series) => series.values)))
const lines = computed(() =>
  props.series.map((series, index) => ({
    ...series,
    line: linePath(series.values, scale.value, WIDTH, HEIGHT),
    area: index === 0 ? areaPath(series.values, scale.value, WIDTH, HEIGHT) : '',
  })),
)
</script>

<template>
  <figure class="flex min-w-0 flex-col gap-2">
    <div class="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
      <span v-for="line in lines" :key="line.name" class="flex items-center gap-1.5">
        <span
          :class="[
            'size-2 rounded-full',
            line.tone === 'destructive' ? 'bg-destructive' : 'bg-foreground',
          ]"
          aria-hidden="true"
        />
        {{ line.name }}
      </span>
      <span class="ml-auto tabular-nums">{{ format(scale) }}</span>
    </div>
    <svg
      :viewBox="`0 0 ${WIDTH} ${HEIGHT}`"
      preserveAspectRatio="none"
      class="h-40 w-full overflow-visible"
      role="img"
      :aria-label="label"
    >
      <line
        v-for="y in [0, HEIGHT / 2, HEIGHT]"
        :key="y"
        x1="0"
        :y1="y"
        :x2="WIDTH"
        :y2="y"
        class="stroke-border"
        stroke-dasharray="2 2"
        vector-effect="non-scaling-stroke"
      />
      <path
        v-for="line in lines.filter((candidate) => candidate.area)"
        :key="`${line.name}-area`"
        :d="line.area"
        class="fill-foreground/10"
      />
      <path
        v-for="line in lines"
        :key="line.name"
        :d="line.line"
        fill="none"
        stroke-width="1.5"
        stroke-linejoin="round"
        vector-effect="non-scaling-stroke"
        :class="line.tone === 'destructive' ? 'stroke-destructive' : 'stroke-foreground'"
      />
    </svg>
    <figcaption class="text-muted-foreground flex justify-between gap-2 text-xs tabular-nums">
      <span>{{ times[0] }}</span>
      <span>{{ times[times.length - 1] }}</span>
    </figcaption>
  </figure>
</template>
