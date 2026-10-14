{{/* Names */}}
{{- define "carry.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "carry.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "carry.selectorLabels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "carry.keySecret" -}}
{{- default (printf "%s-key" (include "carry.fullname" .)) .Values.carry.key.existingSecret -}}
{{- end -}}

{{- define "carry.s3Secret" -}}
{{- default (printf "%s-s3" (include "carry.fullname" .)) .Values.carry.s3Credentials.existingSecret -}}
{{- end -}}

{{/* Path of the carry binary inside the app container. */}}
{{- define "carry.binary" -}}
{{- if .Values.carry.inject.binary -}}{{ .Values.carry.inject.binary }}
{{- else if eq .Values.carry.inject.mode "initContainer" -}}/opt/carry/carry
{{- else -}}/usr/local/bin/carry
{{- end -}}
{{- end -}}

{{/*
Reusable pieces. Another chart that depends on this one can include them in its own
pod spec: carry.initContainers, carry.env, carry.volumeMounts, carry.volumes,
carry.command (the entrypoint prefix for the app container).
*/}}
{{- define "carry.initContainers" -}}
{{- if eq .Values.carry.inject.mode "initContainer" }}
- name: carry-install
  image: "{{ .Values.carry.inject.installerImage.repository }}:{{ .Values.carry.inject.installerImage.tag }}"
  imagePullPolicy: {{ .Values.carry.inject.installerImage.pullPolicy }}
  args: ["/opt/carry"]
  resources:
    requests: {cpu: 10m, memory: 16Mi}
    limits: {memory: 32Mi}
  volumeMounts:
    - {name: carry-bin, mountPath: /opt/carry}
{{- end }}
{{- end -}}

{{- define "carry.command" -}}
- {{ include "carry.binary" . | quote }}
- run
- "--"
{{- end -}}

{{- define "carry.env" -}}
- {name: CARRY_REPO, value: {{ required "carry.repo is required (s3://bucket/prefix)" .Values.carry.repo | quote }}}
- {name: CARRY_STATE, value: {{ required "carry.state is required" .Values.carry.state | quote }}}
{{- with .Values.carry.endpoint }}
- {name: CARRY_S3_ENDPOINT, value: {{ . | quote }}}
{{- end }}
- {name: CARRY_S3_REGION, value: {{ .Values.carry.region | quote }}}
- {name: CARRY_ROOT, value: {{ .Values.carry.root | quote }}}
- {name: CARRY_POLL_SECS, value: {{ .Values.carry.pollSecs | quote }}}
- {name: CARRY_RESTORE_BUDGET_MS, value: {{ .Values.carry.restoreBudgetMs | quote }}}
{{- with .Values.carry.reloadSignal }}
- {name: CARRY_RELOAD_SIGNAL, value: {{ . | quote }}}
{{- end }}
{{- with .Values.carry.toolPubkey }}
- {name: CARRY_TOOL_PUBKEY, value: {{ . | quote }}}
{{- end }}
- {name: CARRY_KEY_FILE, value: {{ printf "%s/%s" .Values.carry.key.mountPath .Values.carry.key.secretKey | quote }}}
- name: AWS_ACCESS_KEY_ID
  valueFrom: {secretKeyRef: {name: {{ include "carry.s3Secret" . }}, key: {{ .Values.carry.s3Credentials.accessKeyIdKey }}}}
- name: AWS_SECRET_ACCESS_KEY
  valueFrom: {secretKeyRef: {name: {{ include "carry.s3Secret" . }}, key: {{ .Values.carry.s3Credentials.secretAccessKeyKey }}}}
{{- end -}}

{{- define "carry.volumeMounts" -}}
- {name: carry-root, mountPath: {{ .Values.carry.root }}}
- {name: carry-key, mountPath: {{ .Values.carry.key.mountPath }}, readOnly: true}
{{- if eq .Values.carry.inject.mode "initContainer" }}
- {name: carry-bin, mountPath: /opt/carry}
{{- end }}
{{- end -}}

{{- define "carry.volumes" -}}
- name: carry-root
  emptyDir: {sizeLimit: {{ .Values.carry.rootSizeLimit }}}
- name: carry-key
  secret:
    secretName: {{ include "carry.keySecret" . }}
    defaultMode: 0400
    items:
      - {key: {{ .Values.carry.key.secretKey }}, path: {{ .Values.carry.key.secretKey }}}
{{- if eq .Values.carry.inject.mode "initContainer" }}
- name: carry-bin
  emptyDir: {sizeLimit: 32Mi}
{{- end }}
{{- end -}}
