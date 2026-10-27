import {expect,it} from 'vitest';
import {assignmentFor,unambiguousSuggestions,type ImportBook} from './importReview';
const book=(id:number,number:number|null):ImportBook=>({id,payloadChapter:1,series:'Series',suggestedEdition:'master',innerPath:`outer.zip!/book-${id}.cbz`,suggestedNumber:number,state:'needs_review',assignedNumber:null,childChapter:null});
it('unknown numbering stays unassigned rather than becoming zero',()=>expect(assignmentFor(book(1,null),{}).number).toBeNull());
it('explicit assignment supersedes suggestions and can choose volume zero',()=>{expect(assignmentFor(book(1,4),{number:5,edition:'deluxe',series:'Other'})).toMatchObject({number:5,edition:'deluxe',series:'Other'});expect(assignmentFor(book(1,null),{number:0}).number).toBe(0);});
it('an explicitly blank edition removes the suggestion',()=>expect(assignmentFor(book(1,4),{edition:''}).edition).toBeNull());
it('duplicate physical alternatives require individual selection',()=>expect(unambiguousSuggestions([book(1,4),book(2,4),book(3,5),book(4,null)]).map(b=>b.id)).toEqual([3]));
it('already queued or ignored siblings are not bulk-approved again',()=>expect(unambiguousSuggestions([{...book(1,1),state:'queued'},{...book(2,2),state:'ignored'},book(3,3)]).map(b=>b.id)).toEqual([3]));
