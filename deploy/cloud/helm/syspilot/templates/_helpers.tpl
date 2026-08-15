{{- define "syspilot.name" -}}syspilot-cloud{{- end }}
{{- define "syspilot.image" -}}
{{- if .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" .Values.image.digest) -}}
{{- fail "image.digest must be a sha256 digest (sha256 followed by 64 lowercase hexadecimal characters)" -}}
{{- end -}}
{{ .Values.image.repository }}@{{ .Values.image.digest }}
{{- else if .Values.image.allowMutableTagForLocalDevelopment -}}
{{ .Values.image.repository }}:{{ required "image.tag is required for the local development override" .Values.image.tag }}
{{- else -}}
{{- fail "image.digest is required for production; allowMutableTagForLocalDevelopment is only for local development" -}}
{{- end -}}
{{- end }}
{{- define "syspilot.labels" -}}
app.kubernetes.io/name: {{ include "syspilot.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}
