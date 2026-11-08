{{- define "locus.name" -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "locus.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "locus.selectorLabels" -}}
app.kubernetes.io/name: {{ include "locus.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "locus.labels" -}}
{{ include "locus.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "locus.large" -}}
{{- if eq .Values.profile "large" -}}true{{- end -}}
{{- end -}}

{{- define "locus.replicas" -}}
{{- if include "locus.large" . -}}{{ .Values.replicaCount }}{{- else -}}1{{- end -}}
{{- end -}}

{{- define "locus.handleDomains" -}}
{{- if .Values.handleDomains -}}
{{- join "," .Values.handleDomains -}}
{{- else -}}
{{- printf ".%s" .Values.hostname -}}
{{- end -}}
{{- end -}}

{{- define "locus.claimName" -}}
{{- .Values.persistence.existingClaim | default (printf "%s-data" (include "locus.fullname" .)) -}}
{{- end -}}

{{/* Refuse values that can't work, before anything is created. */}}
{{- define "locus.validate" -}}
{{- if not .Values.image.repository -}}
{{- fail "image.repository is required: build the image (deploy/docker/Dockerfile) and push it to a registry the cluster can pull from" -}}
{{- end -}}
{{- if not .Values.hostname -}}
{{- fail "hostname is required (the PDS hostname, like pds.example.com)" -}}
{{- end -}}
{{- if not (has .Values.profile (list "small" "large")) -}}
{{- fail "profile must be small or large" -}}
{{- end -}}
{{- if include "locus.large" . -}}
{{- if not .Values.secretEnv.existingSecret -}}
{{- fail "the large profile needs secretEnv.existingSecret, with LOCUS_DATABASE_URL and the master key (LOCUS_MASTER_KEY)" -}}
{{- end -}}
{{- if not .Values.blobs.s3.bucket -}}
{{- fail "the large profile needs blobs.s3.bucket: every pod must read the same blobs" -}}
{{- end -}}
{{- if not (has .Values.masterKeySource (list "env" "kms")) -}}
{{- fail "the large profile needs masterKeySource env or kms, so every pod has the same master key" -}}
{{- end -}}
{{- end -}}
{{- if and .Values.setup.auto (not .Values.setup.keysBackupAcknowledged) -}}
{{- fail "setup.auto needs setup.keysBackupAcknowledged: true (you hold a copy of the master key, or will make a key backup)" -}}
{{- end -}}
{{- if and .Values.setup.auto (not .Values.setup.operatorUsername) -}}
{{- fail "setup.auto needs setup.operatorUsername (and LOCUS_SETUP_OPERATOR_PASSWORD in secretEnv)" -}}
{{- end -}}
{{- end -}}
