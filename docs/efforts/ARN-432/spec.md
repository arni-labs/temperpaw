# Spec
One env line: READY_PATH=/healthz on the deploy job. The driver already
verifies image identity from Railway's record and the live sha from
/paw/version - liveness is the only thing readyz added, and healthz is the
uncoupled liveness surface.
