from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.group_status import GroupStatus
    from ..models.ops_restore_response_200_backup import OpsRestoreResponse200Backup


T = TypeVar("T", bound="OpsRestoreResponse200")


@_attrs_define
class OpsRestoreResponse200:
    """
    Attributes:
        app (str):
        group (int):
        rows (int):
        backup (OpsRestoreResponse200Backup):
        status (GroupStatus): A Raft group's status as one node sees it: the leader (`null`: none known), whether this
            node is it, the term, the last log index (`null`: an empty log), the applied index, and the voters and learners.
    """

    app: str
    group: int
    rows: int
    backup: "OpsRestoreResponse200Backup"
    status: "GroupStatus"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.group_status import GroupStatus
        from ..models.ops_restore_response_200_backup import OpsRestoreResponse200Backup

        app = self.app

        group = self.group

        rows = self.rows

        backup = self.backup.to_dict()

        status = self.status.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "group": group,
                "rows": rows,
                "backup": backup,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.group_status import GroupStatus
        from ..models.ops_restore_response_200_backup import OpsRestoreResponse200Backup

        d = dict(src_dict)
        app = d.pop("app")

        group = d.pop("group")

        rows = d.pop("rows")

        backup = OpsRestoreResponse200Backup.from_dict(d.pop("backup"))

        status = GroupStatus.from_dict(d.pop("status"))

        ops_restore_response_200 = cls(
            app=app,
            group=group,
            rows=rows,
            backup=backup,
            status=status,
        )

        ops_restore_response_200.additional_properties = d
        return ops_restore_response_200

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
