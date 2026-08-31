{{/*
Name helpers (standard Helm boilerplate).
*/}}
{{- define "omnibus.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "omnibus.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "omnibus.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "omnibus.labels" -}}
helm.sh/chart: {{ include "omnibus.chart" . }}
{{ include "omnibus.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "omnibus.selectorLabels" -}}
app.kubernetes.io/name: {{ include "omnibus.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "omnibus.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "omnibus.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
PVC names — honour an existing claim so a migrated install can adopt volumes
that already hold a database.
*/}}
{{- define "omnibus.configClaimName" -}}
{{- if .Values.persistence.config.existingClaim -}}
{{- .Values.persistence.config.existingClaim -}}
{{- else -}}
{{- printf "%s-config" (include "omnibus.fullname" .) -}}
{{- end -}}
{{- end }}

{{- define "omnibus.cacheClaimName" -}}
{{- if .Values.persistence.cache.existingClaim -}}
{{- .Values.persistence.cache.existingClaim -}}
{{- else -}}
{{- printf "%s-cache" (include "omnibus.fullname" .) -}}
{{- end -}}
{{- end }}

{{- define "omnibus.secretName" -}}
{{- if .Values.secrets.existingSecret -}}
{{- .Values.secrets.existingSecret -}}
{{- else -}}
{{- include "omnibus.fullname" . -}}
{{- end -}}
{{- end }}

{{/*
Does the release carry any secret values worth mounting? Keeps an empty Secret
(and a dangling envFrom reference) out of a minimal install.
*/}}
{{- define "omnibus.hasSecrets" -}}
{{- $s := .Values.secrets -}}
{{- if or $s.existingSecret $s.hardcoverApiKey $s.googleBooksApiKey $s.smtp.host -}}
true
{{- end -}}
{{- end }}

{{/*
Scheme for one ingress host: https only if THAT host appears in a TLS block.
A chart-wide "any TLS means https" guess emits the wrong origin for a mixed
ingress, which 403s every write on the plaintext host. Call as:
  include "omnibus.hostScheme" (dict "host" $host.host "root" $)
*/}}
{{- define "omnibus.hostScheme" -}}
{{- $scheme := "http" -}}
{{- range $tls := .root.Values.ingress.tls -}}
{{- if has $.host $tls.hosts -}}
{{- $scheme = "https" -}}
{{- end -}}
{{- end -}}
{{- $scheme -}}
{{- end }}

{{/*
OMNIBUS_PUBLIC_ORIGIN. Explicit value wins; otherwise one origin per ingress
host, each with its own scheme.
*/}}
{{- define "omnibus.publicOrigin" -}}
{{- if .Values.publicOrigin -}}
{{- .Values.publicOrigin -}}
{{- else if .Values.ingress.enabled -}}
{{- $root := . -}}
{{- $origins := list -}}
{{- range $host := .Values.ingress.hosts -}}
{{- $scheme := include "omnibus.hostScheme" (dict "host" $host.host "root" $root) -}}
{{- $origins = append $origins (printf "%s://%s" $scheme $host.host) -}}
{{- end -}}
{{- join "," $origins -}}
{{- end -}}
{{- end }}

{{/*
OMNIBUS_SECURE_COOKIES. Explicit value wins; otherwise on when the derived
origin is https (a Secure cookie is never sent over http, so guessing "on" for
a plaintext install would break login).
*/}}
{{- define "omnibus.secureCookies" -}}
{{- if .Values.secureCookies -}}
{{- .Values.secureCookies -}}
{{- else if hasPrefix "https://" (include "omnibus.publicOrigin" .) -}}
true
{{- else -}}
false
{{- end -}}
{{- end }}
