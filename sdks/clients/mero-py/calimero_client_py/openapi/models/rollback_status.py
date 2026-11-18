from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RollbackStatus")


@_attrs_define
class RollbackStatus:
    """A node's rollback counter: the cluster's epoch (rollbacks forced so far), its generation and mark, whether this
    start was forced (`--accept-rollback`), and the seconds since mero-kms last acknowledged a checkpoint.

        Attributes:
            epoch (int):
            generation (int):
            mark (int):
            forced (bool):
            since_checkpoint_secs (int):
    """

    epoch: int
    generation: int
    mark: int
    forced: bool
    since_checkpoint_secs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        epoch = self.epoch

        generation = self.generation

        mark = self.mark

        forced = self.forced

        since_checkpoint_secs = self.since_checkpoint_secs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "epoch": epoch,
                "generation": generation,
                "mark": mark,
                "forced": forced,
                "sinceCheckpointSecs": since_checkpoint_secs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        epoch = d.pop("epoch")

        generation = d.pop("generation")

        mark = d.pop("mark")

        forced = d.pop("forced")

        since_checkpoint_secs = d.pop("sinceCheckpointSecs")

        rollback_status = cls(
            epoch=epoch,
            generation=generation,
            mark=mark,
            forced=forced,
            since_checkpoint_secs=since_checkpoint_secs,
        )

        rollback_status.additional_properties = d
        return rollback_status

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
