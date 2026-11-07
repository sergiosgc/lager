# Lager

Lager is a web interface for editing an Ansible inventory: the machines, their groupings, the services deployed on them, and the Variables attached to each.

## Language

### Machines

**Host**:
A machine Ansible manages.
_Avoid_: server, node, machine

**Group**:
A named set of Hosts. A Host may belong to many Groups.
_Avoid_: host group, hostgroup

**Parent Group**:
The Group another Group is nested under. A Group has at most one Parent Group, and its Hosts count as members of every Group above it.

**Server Index**:
A sequential number identifying a Host, held in its `server_index` Variable. A new Host gets the next number after the highest existing one unless a different one is given.

### Services

**Service**:
A kind of software that can be deployed, independent of any machine (e.g. `mysql`, `nginx`).

**Parent Service**:
The Service another Service specialises. A Service has at most one Parent Service (e.g. `mariadb` has Parent Service `mysql`).
_Avoid_: base service, super-service

**Service Instance**:
One deployment of a Service on a Host, reachable at its own IP.
_Avoid_: deployment, local service

**Instance Name**:
The name distinguishing Service Instances of the same Service on the same Host (e.g. two `postgresql` clusters on one Host).
_Avoid_: local service name

### Variables

**Variable**:
A named JSON value attached to a Host, Group, Service or Service Instance and handed to Ansible.

**Inheritance**:
A child has every Variable of its parent, and may override any of them with its own value. The nearest definition wins. Hosts inherit from their Groups (and those Groups' Parent Groups); Service Instances inherit from their Service (and its Parent Services).

**Own Variable**:
A Variable set directly on an entity, as opposed to one it inherits.

**Effective Variable**:
The value an entity ends up with once Inheritance is applied, together with where that value came from.
