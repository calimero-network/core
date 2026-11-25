from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.group_status import GroupStatus


T = TypeVar("T", bound="ReplicasSet")


@_attrs_define
class ReplicasSet:
    """What `POST /ops/apps/{app}/replicas` answers: the app group's status after, and whether the set was already the one
    asked for.

        Attributes:
            status (GroupStatus): A Raft group's status as one node sees it: the leader (`null`: none known), whether this
                node is it, the term, the last log index (`null`: an empty log), the applied index, and the voters and learners.
            unchanged (bool):
    """

    status: "GroupStatus"
    unchanged: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.group_status import GroupStatus

        status = self.status.to_dict()

        unchanged = self.unchanged

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "status": status,
                "unchanged": unchanged,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.group_status import GroupStatus

        d = dict(src_dict)
        status = GroupStatus.from_dict(d.pop("status"))

        unchanged = d.pop("unchanged")

        replicas_set = cls(
            status=status,
            unchanged=unchanged,
        )

        replicas_set.additional_properties = d
        return replicas_set

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
