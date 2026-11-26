from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_view_available_type_0_class import UpdateViewAvailableType0Class
from ..models.update_view_available_type_0_source import UpdateViewAvailableType0Source
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.update_reason_type_7 import UpdateReasonType7
    from ..models.update_reason_type_1 import UpdateReasonType1
    from ..models.update_reason_type_6 import UpdateReasonType6
    from ..models.update_view_available_type_0_egress_type_0 import (
        UpdateViewAvailableType0EgressType0,
    )
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2


T = TypeVar("T", bound="UpdateViewAvailableType0")


@_attrs_define
class UpdateViewAvailableType0:
    """
    Attributes:
        package (str):
        version (str):
        source (UpdateViewAvailableType0Source):
        approvals_needed (int):
        class_ (Union[Unset, UpdateViewAvailableType0Class]):
        reasons (Union[Unset, list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2',
            'UpdateReasonType3', 'UpdateReasonType4', 'UpdateReasonType5', 'UpdateReasonType6', 'UpdateReasonType7']]]):
        egress (Union['UpdateViewAvailableType0EgressType0', None, Unset]):
        state_version_changed (Union[None, Unset, bool]):
        new_methods (Union[None, Unset, list[str]]):
        removed_methods (Union[None, Unset, list[str]]):
        signer_changed (Union[None, Unset, bool]):
        summary (Union[Unset, str]):
        approved_by (Union[Unset, str]):
    """

    package: str
    version: str
    source: UpdateViewAvailableType0Source
    approvals_needed: int
    class_: Union[Unset, UpdateViewAvailableType0Class] = UNSET
    reasons: Union[
        Unset,
        list[
            Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
                "UpdateReasonType7",
            ]
        ],
    ] = UNSET
    egress: Union["UpdateViewAvailableType0EgressType0", None, Unset] = UNSET
    state_version_changed: Union[None, Unset, bool] = UNSET
    new_methods: Union[None, Unset, list[str]] = UNSET
    removed_methods: Union[None, Unset, list[str]] = UNSET
    signer_changed: Union[None, Unset, bool] = UNSET
    summary: Union[Unset, str] = UNSET
    approved_by: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_7 import UpdateReasonType7
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_view_available_type_0_egress_type_0 import (
            UpdateViewAvailableType0EgressType0,
        )
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        package = self.package

        version = self.version

        source = self.source.value

        approvals_needed = self.approvals_needed

        class_: Union[Unset, str] = UNSET
        if not isinstance(self.class_, Unset):
            class_ = self.class_.value

        reasons: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.reasons, Unset):
            reasons = []
            for reasons_item_data in self.reasons:
                reasons_item: dict[str, Any]
                if isinstance(reasons_item_data, UpdateReasonType0):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType1):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType2):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType3):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType4):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType5):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType6):
                    reasons_item = reasons_item_data.to_dict()
                else:
                    reasons_item = reasons_item_data.to_dict()

                reasons.append(reasons_item)

        egress: Union[None, Unset, dict[str, Any]]
        if isinstance(self.egress, Unset):
            egress = UNSET
        elif isinstance(self.egress, UpdateViewAvailableType0EgressType0):
            egress = self.egress.to_dict()
        else:
            egress = self.egress

        state_version_changed: Union[None, Unset, bool]
        if isinstance(self.state_version_changed, Unset):
            state_version_changed = UNSET
        else:
            state_version_changed = self.state_version_changed

        new_methods: Union[None, Unset, list[str]]
        if isinstance(self.new_methods, Unset):
            new_methods = UNSET
        elif isinstance(self.new_methods, list):
            new_methods = self.new_methods

        else:
            new_methods = self.new_methods

        removed_methods: Union[None, Unset, list[str]]
        if isinstance(self.removed_methods, Unset):
            removed_methods = UNSET
        elif isinstance(self.removed_methods, list):
            removed_methods = self.removed_methods

        else:
            removed_methods = self.removed_methods

        signer_changed: Union[None, Unset, bool]
        if isinstance(self.signer_changed, Unset):
            signer_changed = UNSET
        else:
            signer_changed = self.signer_changed

        summary = self.summary

        approved_by = self.approved_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "source": source,
                "approvalsNeeded": approvals_needed,
            }
        )
        if class_ is not UNSET:
            field_dict["class"] = class_
        if reasons is not UNSET:
            field_dict["reasons"] = reasons
        if egress is not UNSET:
            field_dict["egress"] = egress
        if state_version_changed is not UNSET:
            field_dict["stateVersionChanged"] = state_version_changed
        if new_methods is not UNSET:
            field_dict["newMethods"] = new_methods
        if removed_methods is not UNSET:
            field_dict["removedMethods"] = removed_methods
        if signer_changed is not UNSET:
            field_dict["signerChanged"] = signer_changed
        if summary is not UNSET:
            field_dict["summary"] = summary
        if approved_by is not UNSET:
            field_dict["approvedBy"] = approved_by

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_7 import UpdateReasonType7
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.update_view_available_type_0_egress_type_0 import (
            UpdateViewAvailableType0EgressType0,
        )
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        source = UpdateViewAvailableType0Source(d.pop("source"))

        approvals_needed = d.pop("approvalsNeeded")

        _class_ = d.pop("class", UNSET)
        class_: Union[Unset, UpdateViewAvailableType0Class]
        if isinstance(_class_, Unset):
            class_ = UNSET
        else:
            class_ = UpdateViewAvailableType0Class(_class_)

        reasons = []
        _reasons = d.pop("reasons", UNSET)
        for reasons_item_data in _reasons or []:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
                "UpdateReasonType7",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_5 = (
                        UpdateReasonType5.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_5
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_6 = (
                        UpdateReasonType6.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_6
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_7 = UpdateReasonType7.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_7

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        def _parse_egress(
            data: object,
        ) -> Union["UpdateViewAvailableType0EgressType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                egress_type_0 = UpdateViewAvailableType0EgressType0.from_dict(data)

                return egress_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UpdateViewAvailableType0EgressType0", None, Unset], data)

        egress = _parse_egress(d.pop("egress", UNSET))

        def _parse_state_version_changed(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        state_version_changed = _parse_state_version_changed(
            d.pop("stateVersionChanged", UNSET)
        )

        def _parse_new_methods(data: object) -> Union[None, Unset, list[str]]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                new_methods_type_0 = cast(list[str], data)

                return new_methods_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, list[str]], data)

        new_methods = _parse_new_methods(d.pop("newMethods", UNSET))

        def _parse_removed_methods(data: object) -> Union[None, Unset, list[str]]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                removed_methods_type_0 = cast(list[str], data)

                return removed_methods_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, list[str]], data)

        removed_methods = _parse_removed_methods(d.pop("removedMethods", UNSET))

        def _parse_signer_changed(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        signer_changed = _parse_signer_changed(d.pop("signerChanged", UNSET))

        summary = d.pop("summary", UNSET)

        approved_by = d.pop("approvedBy", UNSET)

        update_view_available_type_0 = cls(
            package=package,
            version=version,
            source=source,
            approvals_needed=approvals_needed,
            class_=class_,
            reasons=reasons,
            egress=egress,
            state_version_changed=state_version_changed,
            new_methods=new_methods,
            removed_methods=removed_methods,
            signer_changed=signer_changed,
            summary=summary,
            approved_by=approved_by,
        )

        update_view_available_type_0.additional_properties = d
        return update_view_available_type_0

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
