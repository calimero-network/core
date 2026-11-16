from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="OpsRemoveNodeResponse200Status")


@_attrs_define
class OpsRemoveNodeResponse200Status:
    """
    Attributes:
        group (int):
        leader (Union[None, int]):
        is_leader (bool):
        term (int):
        last_log (Union[None, int]):
        applied (int):
        voters (list[int]):
        learners (list[int]):
    """

    group: int
    leader: Union[None, int]
    is_leader: bool
    term: int
    last_log: Union[None, int]
    applied: int
    voters: list[int]
    learners: list[int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        group = self.group

        leader: Union[None, int]
        leader = self.leader

        is_leader = self.is_leader

        term = self.term

        last_log: Union[None, int]
        last_log = self.last_log

        applied = self.applied

        voters = self.voters

        learners = self.learners

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "group": group,
                "leader": leader,
                "is_leader": is_leader,
                "term": term,
                "last_log": last_log,
                "applied": applied,
                "voters": voters,
                "learners": learners,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        group = d.pop("group")

        def _parse_leader(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        leader = _parse_leader(d.pop("leader"))

        is_leader = d.pop("is_leader")

        term = d.pop("term")

        def _parse_last_log(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        last_log = _parse_last_log(d.pop("last_log"))

        applied = d.pop("applied")

        voters = cast(list[int], d.pop("voters"))

        learners = cast(list[int], d.pop("learners"))

        ops_remove_node_response_200_status = cls(
            group=group,
            leader=leader,
            is_leader=is_leader,
            term=term,
            last_log=last_log,
            applied=applied,
            voters=voters,
            learners=learners,
        )

        ops_remove_node_response_200_status.additional_properties = d
        return ops_remove_node_response_200_status

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
