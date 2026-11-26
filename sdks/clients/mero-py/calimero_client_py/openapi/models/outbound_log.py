from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.outbound_audit_entry import OutboundAuditEntry


T = TypeVar("T", bound="OutboundLog")


@_attrs_define
class OutboundLog:
    """An instance's outbound requests, newest first (`entries`), merged from every replica; `partial`: a replica did not
    answer, so its runs' requests may be missing.

        Attributes:
            entries (list['OutboundAuditEntry']):
            partial (bool):
    """

    entries: list["OutboundAuditEntry"]
    partial: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.outbound_audit_entry import OutboundAuditEntry

        entries = []
        for entries_item_data in self.entries:
            entries_item = entries_item_data.to_dict()
            entries.append(entries_item)

        partial = self.partial

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "entries": entries,
                "partial": partial,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.outbound_audit_entry import OutboundAuditEntry

        d = dict(src_dict)
        entries = []
        _entries = d.pop("entries")
        for entries_item_data in _entries:
            entries_item = OutboundAuditEntry.from_dict(entries_item_data)

            entries.append(entries_item)

        partial = d.pop("partial")

        outbound_log = cls(
            entries=entries,
            partial=partial,
        )

        outbound_log.additional_properties = d
        return outbound_log

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
