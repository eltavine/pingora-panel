<script setup lang="ts">
import { ref } from 'vue'
import { Eye, EyeOff } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

const model = defineModel<string>({ required: true })
withDefaults(
  defineProps<{
    id: string
    autocomplete: 'current-password' | 'new-password' | 'off'
    invalid?: boolean
    describedBy?: string
    required?: boolean
  }>(),
  { required: true },
)

const { t } = useI18n()
const visible = ref(false)
</script>

<template>
  <div class="relative">
    <Input
      :id="id"
      v-model="model"
      :type="visible ? 'text' : 'password'"
      :autocomplete="autocomplete"
      :aria-invalid="invalid || undefined"
      :aria-describedby="describedBy"
      class="pr-10"
      :required="required"
    />
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      class="absolute top-1/2 right-1 -translate-y-1/2"
      :aria-label="visible ? t('auth.hidePassword') : t('auth.showPassword')"
      :aria-pressed="visible"
      @click="visible = !visible"
    >
      <EyeOff v-if="visible" aria-hidden="true" />
      <Eye v-else aria-hidden="true" />
    </Button>
  </div>
</template>
