<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { useMutation, useQuery } from '@tanstack/vue-query'
import { Fingerprint, Plus, Save, X } from '@lucide/vue'
import { useI18n } from 'vue-i18n'
import { toast } from 'vue-sonner'
import type { IdentityProviderResponse } from '@/api/generated'
import {
  listRolesOptions,
  putIdentityProviderMutation,
} from '@/api/generated/@tanstack/vue-query.gen'
import CopyValue from '@/components/CopyValue.vue'
import FormField from '@/components/FormField.vue'
import SwitchField from '@/components/SwitchField.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Spinner } from '@/components/ui/spinner'
import { notifyFailure } from '@/lib/configuration'
import PasswordInput from './PasswordInput.vue'
import { callbackUrl, providerForm, providerInput } from './providers'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ provider?: IdentityProviderResponse }>()
const emit = defineEmits<{ saved: [] }>()

const { t } = useI18n()
const roles = useQuery(listRolesOptions())
const save = useMutation(putIdentityProviderMutation())

const form = reactive(providerForm())
watch(open, (isOpen) => {
  if (isOpen) {
    Object.assign(form, providerForm(props.provider))
  }
})

const callback = computed(() => callbackUrl(window.location.origin, form.id || '…'))
const complete = computed(
  () =>
    form.id.trim() &&
    form.displayName.trim() &&
    form.issuer.trim() &&
    form.clientId.trim() &&
    (form.publicClient || form.clientSecret || props.provider?.has_client_secret),
)

function submit() {
  const id = props.provider?.id ?? form.id.trim()
  save.mutate(
    { path: { id }, body: providerInput(form, props.provider) },
    {
      onSuccess: (saved) => {
        toast.success(t('providers.saved', { name: saved.display_name }))
        open.value = false
        emit('saved')
      },
      onError: (error) => notifyFailure(error, t('common.changeFailed')),
    },
  )
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
      <form class="flex flex-col gap-6" @submit.prevent="submit">
        <SheetHeader>
          <SheetTitle>{{ provider ? t('providers.edit') : t('providers.new') }}</SheetTitle>
          <SheetDescription>{{ provider?.id ?? t('providers.description') }}</SheetDescription>
        </SheetHeader>
        <div class="flex flex-col gap-5 px-4">
          <FormField
            v-if="!provider"
            id="provider-id"
            :label="t('providers.id')"
            :hint="t('providers.idHint')"
          >
            <Input
              id="provider-id"
              v-model="form.id"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              pattern="[A-Za-z0-9][A-Za-z0-9._\-]{0,63}"
              required
            />
          </FormField>
          <FormField
            id="provider-name"
            :label="t('providers.name')"
            :hint="t('providers.nameHint')"
          >
            <Input id="provider-name" v-model="form.displayName" maxlength="64" required />
          </FormField>
          <FormField
            id="provider-issuer"
            :label="t('providers.issuer')"
            :hint="t('providers.issuerHint')"
          >
            <Input
              id="provider-issuer"
              v-model="form.issuer"
              type="url"
              autocomplete="off"
              spellcheck="false"
              required
            />
          </FormField>
          <div class="flex flex-col gap-1.5">
            <span class="text-sm font-medium">{{ t('providers.redirectUri') }}</span>
            <CopyValue :value="callback" />
            <p class="text-muted-foreground text-xs">{{ t('providers.redirectHint') }}</p>
          </div>
          <FormField id="provider-client" :label="t('providers.clientId')">
            <Input
              id="provider-client"
              v-model="form.clientId"
              autocomplete="off"
              spellcheck="false"
              required
            />
          </FormField>
          <SwitchField
            id="provider-public"
            v-model="form.publicClient"
            :label="t('providers.publicClient')"
            :hint="t('providers.publicClientHint')"
          />
          <FormField
            v-if="!form.publicClient"
            id="provider-secret"
            :label="t('providers.clientSecret')"
            :hint="provider?.has_client_secret ? t('providers.clientSecretKeep') : undefined"
          >
            <PasswordInput
              id="provider-secret"
              v-model="form.clientSecret"
              autocomplete="new-password"
              :required="!provider?.has_client_secret"
            />
          </FormField>
          <FormField
            id="provider-scopes"
            :label="t('providers.scopes')"
            :hint="t('providers.scopesHint')"
          >
            <Input
              id="provider-scopes"
              v-model="form.scopes"
              autocomplete="off"
              spellcheck="false"
            />
          </FormField>

          <fieldset class="flex flex-col gap-2">
            <legend class="mb-1 text-sm font-medium">{{ t('providers.groupRoles') }}</legend>
            <p class="text-muted-foreground text-xs">{{ t('providers.groupRolesHint') }}</p>
            <div
              v-for="(mapping, index) in form.groupRoles"
              :key="index"
              class="flex items-center gap-2"
            >
              <Input
                v-model="mapping.group"
                :aria-label="t('providers.group')"
                :placeholder="t('providers.group')"
                autocomplete="off"
                spellcheck="false"
                class="min-w-0 flex-1"
              />
              <Select v-model="mapping.role">
                <SelectTrigger :aria-label="t('providers.role')" class="min-w-0 flex-1">
                  <SelectValue :placeholder="t('providers.role')" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="role in roles.data.value ?? []"
                    :key="role.id"
                    :value="role.id"
                  >
                    {{ role.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                :aria-label="t('providers.removeMapping')"
                :title="t('providers.removeMapping')"
                @click="form.groupRoles.splice(index, 1)"
              >
                <X aria-hidden="true" />
              </Button>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="self-start"
              @click="form.groupRoles.push({ group: '', role: '' })"
            >
              <Plus data-icon="inline-start" aria-hidden="true" />
              {{ t('providers.addMapping') }}
            </Button>
          </fieldset>

          <SwitchField
            id="provider-create-accounts"
            v-model="form.createAccounts"
            :label="t('providers.createAccounts')"
            :hint="t('providers.createAccountsHint')"
          />
          <SwitchField
            id="provider-enabled"
            v-model="form.enabled"
            :label="t('providers.enabled')"
            :hint="t('providers.enabledHint')"
          />

          <fieldset class="flex flex-col gap-3">
            <legend class="mb-1 text-sm font-medium">{{ t('providers.claims') }}</legend>
            <FormField id="provider-username-claim" :label="t('providers.usernameClaim')">
              <Input
                id="provider-username-claim"
                v-model="form.usernameClaim"
                autocomplete="off"
                spellcheck="false"
              />
            </FormField>
            <FormField id="provider-groups-claim" :label="t('providers.groupsClaim')">
              <Input
                id="provider-groups-claim"
                v-model="form.groupsClaim"
                autocomplete="off"
                spellcheck="false"
              />
            </FormField>
          </fieldset>
        </div>
        <SheetFooter>
          <Button type="submit" :disabled="save.isPending.value || !complete">
            <Spinner v-if="save.isPending.value" data-icon="inline-start" />
            <Save v-else-if="provider" data-icon="inline-start" aria-hidden="true" />
            <Fingerprint v-else data-icon="inline-start" aria-hidden="true" />
            {{ provider ? t('common.save') : t('common.create') }}
          </Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
