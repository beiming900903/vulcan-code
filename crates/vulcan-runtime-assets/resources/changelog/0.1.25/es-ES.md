Vulcan Code 0.1.25 mejora el inicio de escritorio, las bases de conocimiento y los asistentes dedicados. El instalador independiente 0.2.13 permite elegir la ruta de descarga.

### Inicio y entorno de ejecución

```ui-checklist
ok | Progreso del inicio | La pantalla de inicio muestra la etapa actual de Core y el nombre de la habilidad que se está descargando.
ok | Compatibilidad con LuaSkills | Core usa el entorno y las rutas de Lua incluidos para evitar interferencias de las variables de entorno heredadas. También se corrige la carga de módulos desde rutas de instalación de Windows con caracteres no ASCII.
```

### Bases de conocimiento

```ui-checklist
ok | Reutilización de índices | Cambiar el modo de búsqueda conserva los índices y los ajustes de embeddings. Al reactivar el mismo espacio vectorial, solo se añade el contenido que falta.
ok | Vista previa y resultados | La aplicación web muestra en Markdown los documentos actuales y anteriores, con opción de ver el original. Los resúmenes abren el inicio del documento y las coincidencias de texto abren la línea correspondiente.
```

### Asistentes y modelos

```ui-checklist
ok | Memoria bloqueada | Al convertir una sesión se abre el asistente flotante y se muestra el estado del bloqueo. Los turnos completados con el bloqueo activo indican que su contenido no pasará al turno siguiente.
ok | Instrucciones del sistema | Los cambios en las instrucciones base se aplican antes del historial en la siguiente ejecución, sin actualizar la sesión.
```

### Instalador independiente 0.2.13

```ui-checklist
ok | Rutas de descarga | Elige entre automática, internacional y China continental al instalar o actualizar. El instalador recuerda la selección y admite `--route`.
```
