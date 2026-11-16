from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.group_status import GroupStatus
    from ..models.governance import Governance
    from ..models.rollback_status import RollbackStatus
    from ..models.node_record import NodeRecord
    from ..models.ops_status_response_200_groups_item import (
        OpsStatusResponse200GroupsItem,
    )


T = TypeVar("T", bound="OpsStatusResponse200")


@_attrs_define
class OpsStatusResponse200:
    """
    Attributes:
        node (int):
        system (GroupStatus): A Raft group's status as one node sees it: the leader (`null`: none known), whether this
            node is it, the term, the last log index (`null`: an empty log), the applied index, and the voters and learners.
        governance (Governance): Who decides what on this cluster: release approvals, app upgrades and outbound
            approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments`
            (`open`), `deployers` (`operator`) and `signUp` (`open`).
        groups (list['OpsStatusResponse200GroupsItem']):
        nodes (list['NodeRecord']):
        apps (int):
        accounts (int):
        rollback (Union['RollbackStatus', None]):
    """

    node: int
    system: "GroupStatus"
    governance: "Governance"
    groups: list["OpsStatusResponse200GroupsItem"]
    nodes: list["NodeRecord"]
    apps: int
    accounts: int
    rollback: Union["RollbackStatus", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.group_status import GroupStatus
        from ..models.governance import Governance
        from ..models.rollback_status import RollbackStatus
        from ..models.node_record import NodeRecord
        from ..models.ops_status_response_200_groups_item import (
            OpsStatusResponse200GroupsItem,
        )

        node = self.node

        system = self.system.to_dict()

        governance = self.governance.to_dict()

        groups = []
        for groups_item_data in self.groups:
            groups_item = groups_item_data.to_dict()
            groups.append(groups_item)

        nodes = []
        for nodes_item_data in self.nodes:
            nodes_item = nodes_item_data.to_dict()
            nodes.append(nodes_item)

        apps = self.apps

        accounts = self.accounts

        rollback: Union[None, dict[str, Any]]
        if isinstance(self.rollback, RollbackStatus):
            rollback = self.rollback.to_dict()
        else:
            rollback = self.rollback

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "node": node,
                "system": system,
                "governance": governance,
                "groups": groups,
                "nodes": nodes,
                "apps": apps,
                "accounts": accounts,
                "rollback": rollback,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.group_status import GroupStatus
        from ..models.governance import Governance
        from ..models.rollback_status import RollbackStatus
        from ..models.node_record import NodeRecord
        from ..models.ops_status_response_200_groups_item import (
            OpsStatusResponse200GroupsItem,
        )

        d = dict(src_dict)
        node = d.pop("node")

        system = GroupStatus.from_dict(d.pop("system"))

        governance = Governance.from_dict(d.pop("governance"))

        groups = []
        _groups = d.pop("groups")
        for groups_item_data in _groups:
            groups_item = OpsStatusResponse200GroupsItem.from_dict(groups_item_data)

            groups.append(groups_item)

        nodes = []
        _nodes = d.pop("nodes")
        for nodes_item_data in _nodes:
            nodes_item = NodeRecord.from_dict(nodes_item_data)

            nodes.append(nodes_item)

        apps = d.pop("apps")

        accounts = d.pop("accounts")

        def _parse_rollback(data: object) -> Union["RollbackStatus", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                rollback_type_0 = RollbackStatus.from_dict(data)

                return rollback_type_0
            except:  # noqa: E722
                pass
            return cast(Union["RollbackStatus", None], data)

        rollback = _parse_rollback(d.pop("rollback"))

        ops_status_response_200 = cls(
            node=node,
            system=system,
            governance=governance,
            groups=groups,
            nodes=nodes,
            apps=apps,
            accounts=accounts,
            rollback=rollback,
        )

        ops_status_response_200.additional_properties = d
        return ops_status_response_200

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
