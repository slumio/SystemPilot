{{- define "syspilot.name" -}}syspilot-cloud{{- end }}
{{- define "syspilot.labels" -}}
app.kubernetes.io/name: {{ include "syspilot.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}
