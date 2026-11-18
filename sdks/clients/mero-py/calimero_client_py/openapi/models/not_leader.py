from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="NotLeader")


@_attrs_define
class NotLeader:
    """`details` of a `421 NOT_LEADER`: the leader's URL to send the same request to, and its node id where known.

    Attributes:
        leader_url (str):
        leader (Union[Unset, int]):
    """

    leader_url: str
    leader: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        leader_url = self.leader_url

        leader = self.leader

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "leaderUrl": leader_url,
            }
        )
        if leader is not UNSET:
            field_dict["leader"] = leader

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        leader_url = d.pop("leaderUrl")

        leader = d.pop("leader", UNSET)

        not_leader = cls(
            leader_url=leader_url,
            leader=leader,
        )

        not_leader.additional_properties = d
        return not_leader

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
