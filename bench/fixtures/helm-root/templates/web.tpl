apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  annotations:
    replicas: {{ .Values.replicas | quote }}
spec:
  template:
    metadata:
      labels: {app: web}
    spec:
      containers:
      - name: web
        image: {{ .Values.global.registry }}/web:1
        env:
        {{- include "shop.env" . | nindent 8 }}
        {{- if .Values.flag }}
        - name: FLAG_ON
          value: "1"
        {{- end }}
